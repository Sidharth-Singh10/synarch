use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use serde::Serialize;
use std::{env, fs, path::PathBuf, thread, time::Duration};
use synarch::{
    contracts::Phase,
    controller::{self, Config, StepOutcome},
    opencode::{AgentBackend, OpenCode},
    recovery,
    store::Store,
};

#[derive(Parser)]
#[command(
    version,
    about = "Evidence-gated multi-agent development, powered by OpenCode V2"
)]
struct Cli {
    #[arg(long, global = true, default_value_os_t = default_database())]
    db: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve a read-only dashboard on IPv4 loopback.
    Dashboard {
        #[arg(long, default_value_t = 8787)]
        port: u16,
    },
    /// Journal a new request. Does not launch agents or modify the source checkout.
    Create {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        request: String,
    },
    /// Advance one stage or poll the existing agent attempt. Requires a configured server.
    Step { run_id: String },
    /// Advance until ready/escalated, an error, or the polling limit.
    Run {
        run_id: String,
        #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u32).range(1..=10000))]
        max_polls: u32,
    },
    /// Read persisted state. Ready is tied to the recorded commit, not a deployment approval.
    Status { run_id: String },
    /// Read the durable decision/evidence timeline as JSON Lines.
    Events { run_id: String },
    /// Pause future scheduling; already-running OpenCode sessions are not interrupted.
    Pause { run_id: String },
    /// Resume future scheduling, retaining the same in-flight assignment IDs.
    Resume { run_id: String },
    /// Inspect attempts, frozen dispatches, failure states, and stop confirmation.
    Attempts { run_id: String },
    /// Explicitly recover a blocked attempt or retry a confirmed stopped failure.
    Retry {
        run_id: String,
        #[arg(long)]
        reason: String,
    },
    /// Terminal cancellation. Retains worktrees and journals intent before interruption.
    Cancel {
        run_id: String,
        #[arg(long)]
        reason: String,
    },
    /// Resolve escalation without overriding acceptance/QA gates. Extensions are explicit.
    Resolve {
        run_id: String,
        #[arg(long)]
        reason: String,
        #[arg(long, default_value_t = 0)]
        additional_seconds: u64,
        #[arg(long, default_value_t = 0)]
        additional_launches: u32,
        #[arg(long, default_value_t = 0)]
        additional_iterations: u32,
    },
}

fn default_database() -> PathBuf {
    env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".synarch"))
        .join("synarch/state.sqlite")
}

fn backend() -> Result<OpenCode> {
    let url = env::var("SYNARCH_OPENCODE_URL")
        .context("set SYNARCH_OPENCODE_URL to your OpenCode V2 server root URL")?;
    let authorization = env::var("SYNARCH_OPENCODE_AUTHORIZATION").ok();
    OpenCode::new(&url, authorization)
}

fn optional_backend() -> Result<Option<OpenCode>> {
    if env::var_os("SYNARCH_OPENCODE_URL").is_none() {
        return Ok(None);
    }
    backend().map(Some)
}

fn print(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string(value)?);
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Dashboard { port } => synarch::dashboard::serve(&cli.db, port)?,
        Command::Create { config, request } => {
            let path = config
                .canonicalize()
                .context("configuration file not found")?;
            let contents = fs::read(&path)?;
            ensure!(contents.len() <= 262_144, "configuration exceeds 256 KiB");
            let mut config: Config = serde_json::from_slice(&contents)?;
            let parent = path.parent().context("configuration directory required")?;
            if config.repository.is_relative() {
                config.repository = parent.join(config.repository);
            }
            if config.workspace_root.is_relative() {
                config.workspace_root = parent.join(config.workspace_root);
            }
            print(&controller::create_run(&cli.db, config, request)?)?;
        }
        Command::Step { run_id } => {
            print(&controller::step(&cli.db, &run_id, &backend()?)?)?;
        }
        Command::Run { run_id, max_polls } => {
            let backend = backend()?;
            for _ in 0..max_polls {
                let outcome = controller::step(&cli.db, &run_id, &backend)?;
                print(&outcome)?;
                match outcome {
                    StepOutcome::Advanced { run } | StepOutcome::Terminal { run } => {
                        if run.phase == Phase::Ready {
                            return Ok(());
                        }
                        if matches!(run.phase, Phase::Escalated | Phase::Cancelled) {
                            bail!(
                                "run is escalated or cancelled; inspect attempts and the event ledger"
                            );
                        }
                    }
                    StepOutcome::Pending { .. } => thread::sleep(Duration::from_secs(1)),
                }
            }
            bail!(
                "polling limit reached; active agents may continue. Resume with the same run ID, or pause scheduling"
            );
        }
        Command::Status { run_id } => {
            let store = existing_store(&cli.db)?;
            let mut status = serde_json::to_value(store.load(&run_id)?)?;
            status["execution_control"] = serde_json::to_value(store.control(&run_id)?)?;
            print(&status)?;
        }
        Command::Events { run_id } => {
            let store = existing_store(&cli.db)?;
            store.load(&run_id)?;
            for event in store.events(&run_id)? {
                print(&event)?;
            }
        }
        Command::Pause { run_id } => {
            let mut store = existing_store(&cli.db)?;
            store.set_paused(&run_id, true)?;
            print(&store.load(&run_id)?)?;
        }
        Command::Resume { run_id } => {
            let mut store = existing_store(&cli.db)?;
            store.set_paused(&run_id, false)?;
            print(&store.load(&run_id)?)?;
        }
        Command::Attempts { run_id } => {
            let store = existing_store(&cli.db)?;
            store.load(&run_id)?;
            for attempt in store.attempts(&run_id)? {
                print(&attempt)?;
            }
        }
        Command::Retry { run_id, reason } => {
            let backend = optional_backend()?;
            print(&recovery::retry(
                &cli.db,
                &run_id,
                &reason,
                backend.as_ref().map(|b| b as &dyn AgentBackend),
            )?)?;
        }
        Command::Cancel { run_id, reason } => {
            let backend = optional_backend()?;
            print(&recovery::cancel(
                &cli.db,
                &run_id,
                &reason,
                backend.as_ref().map(|b| b as &dyn AgentBackend),
            )?)?;
        }
        Command::Resolve {
            run_id,
            reason,
            additional_seconds,
            additional_launches,
            additional_iterations,
        } => {
            let backend = optional_backend()?;
            print(&recovery::resolve(
                &cli.db,
                &run_id,
                &reason,
                additional_seconds,
                additional_launches,
                additional_iterations,
                backend.as_ref().map(|b| b as &dyn AgentBackend),
            )?)?;
        }
    }
    Ok(())
}

fn existing_store(path: &std::path::Path) -> Result<Store> {
    ensure!(
        path.is_file(),
        "state database does not exist; create a run first"
    );
    Store::open(path)
}
