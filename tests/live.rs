//! Opt-in live-model tests. Never run by `cargo test` without `--ignored`.
//!
//! Required environment:
//!   SYNARCH_LIVE_CONFIRM=1   explicit acknowledgement of provider usage
//!   SYNARCH_OPENCODE_URL     OpenCode V2 server root
//!   SYNARCH_OPENCODE_AUTHORIZATION (optional) full Authorization header value
//!   SYNARCH_LIVE_MODEL       provider/model, default opencode-go/deepseek-v4.1-flash
//!
//! Use `scripts/live-rework.sh` or `scripts/live-cancel.sh`, which populate the
//! local service connection without printing credentials.

use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::{Duration, Instant},
};
use synarch::{
    contracts::*,
    controller::{Config, Profile, Profiles, StepOutcome, create_run, step},
    opencode::{AgentBackend, Model, OpenCode, WorkerOutcome, WorkerState},
    recovery,
    runtime::AttemptState,
    store::Store,
    workspace::CheckCommand,
};

/// On failure, still attempt to stop only this run's owned worker.
struct CancellationGuard<'a> {
    database: &'a Path,
    run_id: &'a str,
    backend: &'a OpenCode,
    armed: bool,
}

impl Drop for CancellationGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            for _ in 0..5 {
                if recovery::cancel(
                    self.database,
                    self.run_id,
                    "live harness cleanup",
                    Some(self.backend),
                )
                .is_ok()
                {
                    return;
                }
                thread::sleep(Duration::from_millis(500));
            }
            eprintln!(
                "WARNING: cleanup could not confirm worker termination; inspect {}",
                self.database.display()
            );
        }
    }
}

#[test]
#[ignore = "live model calls; run scripts/live-cancel.sh"]
fn live_interrupts_actively_generating_worker() {
    assert_eq!(std::env::var("SYNARCH_LIVE_CONFIRM").as_deref(), Ok("1"));
    let root = live_root("synarch-live-cancel.");
    let policy_root = live_root("synarch-live-cancel-policy.");
    let repo = fixture(&root);
    let config = config(&root, repo, write_policy(&policy_root));
    let database = root.join("state.sqlite");
    let run = create_run(&database, config, "Develop a detailed, comprehensive implementation plan with many independent acceptance criteria for improving the parity function, covering all integer edge cases, regression testing, accessibility of documentation, and maintaining the public API. Do not execute changes.".into()).unwrap();
    let url = std::env::var("SYNARCH_OPENCODE_URL").unwrap();
    let authorization = std::env::var("SYNARCH_OPENCODE_AUTHORIZATION").ok();
    let backend = OpenCode::new(&url, authorization.clone()).unwrap();
    let mut guard = CancellationGuard {
        database: &database,
        run_id: &run.id,
        backend: &backend,
        armed: true,
    };
    eprintln!("cancellation evidence: {}", root.display());

    // The controller launches and polls for at most two seconds, releasing its lock.
    let result = step(&database, &run.id, &backend).unwrap();
    assert!(
        matches!(result, StepOutcome::Pending { .. }),
        "worker completed before observation"
    );
    let store = Store::open(&database).unwrap();
    let attempt = store.attempts(&run.id).unwrap().pop().unwrap();
    assert_eq!(attempt.attempt.role, Role::Jury);
    assert_eq!(attempt.state, AttemptState::Running);
    assert!(!attempt.stop_confirmed);
    drop(store);

    // A queued input or active session alone does not prove model generation.
    // Require an unfinished assistant message with nonempty streamed text/reasoning.
    let client = reqwest::blocking::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let started = Instant::now();
    let mut observed = false;
    let mut last_state = WorkerState::Missing;
    while started.elapsed() < Duration::from_secs(45) {
        let mut request = client.get(format!(
            "{}/api/session/{}/message?type=assistant&order=desc&limit=1",
            url.trim_end_matches('/'),
            attempt.attempt.session_id
        ));
        if let Some(auth) = &authorization {
            request = request.header(reqwest::header::AUTHORIZATION, auth);
        }
        let message: Value = request
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap();
        let assistant = &message["data"][0];
        let streamed = assistant["content"].as_array().is_some_and(|parts| {
            parts.iter().any(|part| {
                matches!(part["type"].as_str(), Some("text" | "reasoning"))
                    && part["text"].as_str().is_some_and(|s| !s.is_empty())
            })
        });
        last_state = backend.inspect(&attempt.attempt).unwrap();
        if assistant["type"] == "assistant"
            && assistant["finish"].is_null()
            && streamed
            && last_state == WorkerState::Running
        {
            observed = true;
            break;
        }
        if matches!(last_state, WorkerState::Stopped { .. }) {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    assert!(
        observed,
        "no actively streaming assistant output observed (last state: {last_state:?}); evidence at {}",
        root.display()
    );
    eprintln!("observed unfinished assistant output while worker was active");

    let mut cancelled = false;
    for _ in 0..10 {
        if recovery::cancel(
            &database,
            &run.id,
            "verify active generation cancellation",
            Some(&backend),
        )
        .is_ok()
        {
            cancelled = true;
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    assert!(
        cancelled,
        "cancellation not confirmed; evidence at {}",
        root.display()
    );
    let final_state = backend.inspect(&attempt.attempt).unwrap();
    assert_eq!(
        final_state,
        WorkerState::Stopped {
            outcome: WorkerOutcome::Interrupted
        },
        "worker may have finished before interrupt"
    );
    let store = Store::open(&database).unwrap();
    assert_eq!(store.load(&run.id).unwrap().phase, Phase::Cancelled);
    let final_attempt = store.attempts(&run.id).unwrap().pop().unwrap();
    assert_eq!(final_attempt.state, AttemptState::Interrupted);
    assert!(final_attempt.stop_confirmed);
    let events = store.events(&run.id).unwrap();
    let intent = events
        .iter()
        .position(|e| e.kind == "stop_requested")
        .unwrap();
    let confirmed = events
        .iter()
        .position(|e| e.kind == "worker_stop_observed")
        .unwrap();
    assert!(intent < confirmed);
    drop(store);
    assert!(matches!(
        step(&database, &run.id, &backend).unwrap(),
        StepOutcome::Terminal { .. }
    ));
    // A brief observation window checks for an immediate restart or late transition;
    // it does not prove the remote provider stopped billing or computing.
    thread::sleep(Duration::from_secs(2));
    assert_eq!(backend.inspect(&attempt.attempt).unwrap(), final_state);
    let store = Store::open(&database).unwrap();
    let final_run = store.load(&run.id).unwrap();
    assert_eq!(final_run.phase, Phase::Cancelled);
    assert_eq!(final_run.revision, run.revision);
    assert_eq!(store.attempts(&run.id).unwrap().len(), 1);
    assert!(
        !store
            .events(&run.id)
            .unwrap()
            .iter()
            .any(|e| e.kind == "transition")
    );
    drop(store);
    guard.armed = false;
    eprintln!(
        "confirmed interrupted and quiescent: session {}",
        attempt.attempt.session_id
    );
}

fn git(path: &Path, args: &[&str]) {
    let result = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .expect("Git must be installed");
    assert!(
        result.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&result.stderr)
    );
}

fn model() -> Model {
    let reference = std::env::var("SYNARCH_LIVE_MODEL")
        .unwrap_or_else(|_| "opencode-go/deepseek-v4.1-flash".into());
    let (provider_id, id) = reference
        .split_once('/')
        .unwrap_or_else(|| panic!("SYNARCH_LIVE_MODEL must be provider/model, got {reference:?}"));
    assert!(
        !provider_id.is_empty() && !id.is_empty(),
        "invalid model reference"
    );
    Model {
        provider_id: provider_id.into(),
        id: id.into(),
        variant: std::env::var("SYNARCH_LIVE_VARIANT").ok(),
    }
}

fn live_root(prefix: &str) -> PathBuf {
    let base = Path::new("/tmp/opencode");
    let directory = if base.is_dir() {
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in(base)
            .expect("create live fixture under /tmp/opencode")
    } else {
        tempfile::Builder::new()
            .prefix(prefix)
            .tempdir()
            .expect("create live fixture")
    };
    // Keep the ledger and worktrees after a panic so a failed live run is diagnosable.
    directory.keep()
}

/// Controller-owned checks live outside both the source checkout and its ancestor
/// directories, so an agent exploring its workspace cannot stumble onto them.
fn write_policy(directory: &Path) -> PathBuf {
    let script = directory.join("policy-check.sh");
    fs::write(
        &script,
        r#"#!/bin/sh
# Controller-owned policy gate. Not part of the source checkout.
if git log -1 --format=%B | grep -qx 'Refs: POLICY-42'; then
    exit 0
fi
echo "policy: add a 'Refs: POLICY-42' trailer to the current commit message (git commit --amend)" >&2
exit 1
"#,
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    script
}

/// A disposable fixture whose project policy is intentionally not mentioned in
/// the user request. The Actor does not see check results; the Jury must explain
/// the actionable failure before the Actor can fix it.
fn fixture(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(
        &repo,
        &["config", "user.email", "synarch-live@example.invalid"],
    );
    git(&repo, &["config", "user.name", "Synarch live fixture"]);
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"synarch-live-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n",
    )
    .unwrap();
    fs::write(repo.join(".gitignore"), "/target/\n").unwrap();
    fs::write(
        repo.join("src/lib.rs"),
        r#"pub fn is_even(value: i32) -> bool {
    value % 2 == 1
}

#[cfg(test)]
mod tests {
    use super::is_even;

    #[test]
    fn recognizes_positive_even_numbers() {
        assert!(is_even(2));
        assert!(!is_even(3));
    }
}
"#,
    )
    .unwrap();
    // `--locked` must be meaningful from the first Actor pass; commit the real lockfile.
    let lock = Command::new("cargo")
        .arg("generate-lockfile")
        .current_dir(&repo)
        .output()
        .expect("cargo must be installed");
    assert!(
        lock.status.success(),
        "cargo generate-lockfile: {}",
        String::from_utf8_lossy(&lock.stderr)
    );
    fs::write(
        repo.join("opencode.json"),
        serde_json::to_string_pretty(&json!({
            "$schema": "https://opencode.ai/config.json",
            "agents": {
                "synarch-jury": {"mode":"all","description":"Technical planner and acceptance authority","steps":12,
                    "permissions":[{"action":"*","resource":"*","effect":"deny"}]},
                "synarch-actor": {"mode":"all","description":"Implements controller-assigned changes","steps":16,
                    "permissions":[{"action":"*","resource":"*","effect":"deny"}]},
                "synarch-critic": {"mode":"all","description":"Independent read-only reviewer","steps":12,
                    "permissions":[{"action":"*","resource":"*","effect":"deny"}]},
                "synarch-qa": {"mode":"all","description":"Independent behavioral verifier","steps":16,
                    "permissions":[{"action":"*","resource":"*","effect":"deny"}]}
            }
        }))
        .unwrap(),
    )
    .unwrap();
    git(
        &repo,
        &[
            "add",
            "Cargo.toml",
            "Cargo.lock",
            ".gitignore",
            "src/lib.rs",
            "opencode.json",
        ],
    );
    git(
        &repo,
        &[
            "commit",
            "-qm",
            "test: add intentionally failing parity fixture",
        ],
    );
    repo
}

fn config(root: &Path, repo: PathBuf, script: PathBuf) -> Config {
    let model = model();
    Config {
        repository: repo,
        workspace_root: root.join("worktrees"),
        max_iterations: 2,
        checks: vec![
            CheckCommand {
                name: "tests".into(),
                argv: vec!["cargo".into(), "test".into(), "--locked".into()],
                timeout_seconds: 90,
            },
            CheckCommand {
                name: "policy".into(),
                argv: vec!["sh".into(), script.to_string_lossy().into_owned()],
                timeout_seconds: 10,
            },
        ],
        agents: Profiles {
            jury: Profile {
                agent: "synarch-jury".into(),
                model: Some(model.clone()),
            },
            actor: Profile {
                agent: "synarch-actor".into(),
                model: Some(model.clone()),
            },
            critic: Profile {
                agent: "synarch-critic".into(),
                model: Some(model.clone()),
            },
            qa: Profile {
                agent: "synarch-qa".into(),
                model: Some(model),
            },
        },
        limits: synarch::runtime::Limits {
            worker_timeout_seconds: 600,
            run_timeout_seconds: 1800,
            max_agent_launches: 12,
            max_attempts_per_stage: 2,
        },
    }
}

fn request() -> String {
    "Fix is_even(i32) so it returns true exactly for even integers, including zero and negative integers. \
     Preserve the public function signature. Add regression tests for zero, negative odd and even values, \
     and i32::MIN/i32::MAX. Do not change Cargo.toml, Cargo.lock, OpenCode configuration, or add dependencies. \
     Keep the implementation minimal. Create a local Conventional Commit and leave the worktree clean; \
     do not push or deploy."
        .into()
}

fn transition<'a>(events: &'a [synarch::store::Event], to: &str) -> Vec<&'a Value> {
    events
        .iter()
        .filter(|event| event.kind == "transition" && event.payload["to"] == to)
        .map(|event| &event.payload)
        .collect()
}

fn dump(store: &Store, id: &str) {
    eprintln!("run: {id}");
    eprintln!(
        "state: {}",
        serde_json::to_string_pretty(&store.load(id).unwrap()).unwrap()
    );
    for event in store.events(id).unwrap() {
        eprintln!("{}", serde_json::to_string(&event).unwrap());
    }
    for attempt in store.attempts(id).unwrap() {
        eprintln!("{}", serde_json::to_string(&attempt).unwrap());
    }
}

#[test]
#[ignore = "live model calls; run scripts/live-rework.sh"]
fn live_policy_failure_forces_rework_then_repeats_all_gates() {
    assert_eq!(
        std::env::var("SYNARCH_LIVE_CONFIRM").as_deref(),
        Ok("1"),
        "set SYNARCH_LIVE_CONFIRM=1 to acknowledge provider usage"
    );
    let root = live_root("synarch-live-rework.");
    let policy_root = live_root("synarch-live-policy.");
    let repo = fixture(root.as_path());
    let script = write_policy(policy_root.as_path());
    let database = root.as_path().join("state.sqlite");
    let config = config(root.as_path(), repo, script);
    let run = create_run(&database, config, request()).unwrap();
    let url = std::env::var("SYNARCH_OPENCODE_URL").expect("set SYNARCH_OPENCODE_URL");
    let authorization = std::env::var("SYNARCH_OPENCODE_AUTHORIZATION").ok();
    let backend = OpenCode::new(&url, authorization).unwrap();

    let started = Instant::now();
    let mut terminal: Option<Run> = None;
    for _ in 0..900 {
        match step(&database, &run.id, &backend) {
            Ok(StepOutcome::Terminal { run }) => {
                terminal = Some(run);
                break;
            }
            Ok(StepOutcome::Advanced { run }) => {
                eprintln!(
                    "advanced: phase={:?} revision={} iteration={} snapshot={:?}",
                    run.phase, run.revision, run.iteration, run.snapshot
                );
            }
            Ok(StepOutcome::Pending { .. }) => {}
            Err(error) => {
                let store = Store::open(&database).unwrap();
                let current = store.load(&run.id).unwrap();
                if matches!(current.phase, Phase::Escalated | Phase::Cancelled) {
                    eprintln!("terminal after error: {error}");
                    terminal = Some(current);
                    break;
                }
                dump(&store, &run.id);
                panic!(
                    "live workflow failed after {:?}: {error:#}",
                    started.elapsed()
                );
            }
        }
        thread::sleep(Duration::from_secs(1));
    }

    let store = Store::open(&database).unwrap();
    let run = terminal.unwrap_or_else(|| {
        dump(&store, &run.id);
        panic!("live workflow did not reach a terminal state within 15 minutes")
    });
    let events = store.events(&run.id).unwrap();
    eprintln!(
        "result: phase={:?} revision={} iteration={} snapshot={:?} elapsed={:?}",
        run.phase,
        run.revision,
        run.iteration,
        run.snapshot,
        started.elapsed()
    );

    assert_eq!(
        run.phase,
        Phase::Ready,
        "live rework did not converge; inspect the ledger at {}",
        database.display()
    );
    assert!(run.engineering_accepted && run.qa_validated);
    assert!(
        run.checks.len() == run.mandatory_checks.len()
            && run.checks.iter().all(|check| check.passed),
        "final acceptance requires every mandatory check to pass"
    );

    let check_reports = events
        .iter()
        .filter(|event| {
            event.kind == "transition" && event.payload["submission"]["report"]["kind"] == "checks"
        })
        .map(|event| &event.payload)
        .collect::<Vec<_>>();
    assert!(
        check_reports.len() >= 2,
        "expected at least two check runs around rework"
    );
    let first = &check_reports[0]["submission"]["report"]["results"];
    let policy = first
        .as_array()
        .unwrap()
        .iter()
        .find(|result| result["name"] == "policy")
        .expect("policy check result");
    assert_eq!(
        policy["passed"], false,
        "first pass must fail the hidden policy gate"
    );
    assert!(
        policy["evidence"]
            .as_str()
            .unwrap()
            .contains("Refs: POLICY-42"),
        "policy evidence must be actionable: {policy}"
    );

    let rework = transition(&events, "implementing")
        .into_iter()
        .any(|payload| payload["submission"]["report"]["verdict"] == "rework");
    assert!(
        rework,
        "a failed mandatory check must be corrected through rework"
    );

    let final_checks = run
        .checks
        .iter()
        .map(|check| check.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(final_checks, vec!["tests", "policy"]);
    assert!(run.checks.iter().all(|check| check.passed));

    let qa = transition(&events, "qa");
    let ready = transition(&events, "ready");
    assert_eq!(qa.len(), 1, "exactly one accepted snapshot should reach QA");
    assert_eq!(ready.len(), 1);
    assert_eq!(
        qa[0]["submission"]["snapshot"], ready[0]["submission"]["snapshot"],
        "QA and readiness must refer to the same accepted snapshot"
    );

    let implementations = transition(&events, "checking")
        .into_iter()
        .filter_map(|payload| payload["submission"]["snapshot"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        implementations.len(),
        2,
        "one implementation and one rework snapshot"
    );
    assert_ne!(
        implementations[0], implementations[1],
        "rework must produce a new reviewed snapshot"
    );

    let actor_attempts = store
        .attempts(&run.id)
        .unwrap()
        .into_iter()
        .filter(|attempt| attempt.attempt.role == Role::Actor)
        .count();
    assert_eq!(actor_attempts, 2, "rework needs a fresh Actor attempt");

    // Every acceptance decision must be bound to the final passing snapshot.
    let accepted = events
        .iter()
        .filter(|event| {
            event.kind == "transition"
                && event.payload["submission"]["report"]["verdict"] == "accept"
        })
        .count();
    assert!(
        accepted >= 1,
        "the workflow must record an explicit acceptance"
    );
    for event in &events {
        if event.kind == "transition"
            && event.payload["submission"]["report"]["verdict"] == "accept"
        {
            let accepted_checks = event.payload["submission"]["snapshot"].as_str();
            assert_eq!(
                accepted_checks,
                run.snapshot.as_deref(),
                "acceptance must be bound to the final accepted snapshot"
            );
        }
    }

    fs::write(
        root.as_path().join("result.json"),
        serde_json::to_string_pretty(&json!({
            "run": run,
            "elapsed_seconds": started.elapsed().as_secs(),
            "actor_attempts": actor_attempts,
            "check_runs": check_reports.len(),
        }))
        .unwrap(),
    )
    .unwrap();
    eprintln!(
        "evidence roots: {} and {}",
        root.as_path().display(),
        policy_root.as_path().display()
    );
}
