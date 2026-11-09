use crate::contracts::{CheckResult, text};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckCommand {
    pub name: String,
    /// Trusted operator configuration, never a model-proposed command string.
    pub argv: Vec<String>,
    pub timeout_seconds: u64,
}

impl CheckCommand {
    pub fn validate(&self) -> Result<()> {
        text(&self.name)?;
        ensure!(
            !self.argv.is_empty() && !self.argv[0].trim().is_empty(),
            "check executable required"
        );
        ensure!(
            self.argv.iter().map(String::len).sum::<usize>() <= 8192,
            "check command too long"
        );
        ensure!(
            (1..=3600).contains(&self.timeout_seconds),
            "check timeout must be 1..=3600 seconds"
        );
        Ok(())
    }
}

pub fn repository(path: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git(path, &["rev-parse", "--show-toplevel"])?).canonicalize()?)
}

/// A snapshot includes all committed tracked files. Ignored build outputs are not source artifacts.
pub fn snapshot(path: &Path) -> Result<String> {
    ensure!(
        git(path, &["status", "--porcelain=v1", "--untracked-files=all"])?.is_empty(),
        "workspace contains uncommitted changes; commit implementation before reporting completion"
    );
    head(path)
}

pub fn head(path: &Path) -> Result<String> {
    git(path, &["rev-parse", "--verify", "HEAD^{commit}"])
}

/// Supply a real integrated diff to read-only reviewers, without enabling shell access.
pub fn diff(path: &Path, baseline: &str, snapshot: &str) -> Result<String> {
    for commit in [baseline, snapshot] {
        ensure!(
            matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit()),
            "full Git commit ID required"
        );
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(path)
        .args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            baseline,
            snapshot,
            "--",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .context("missing Git diff pipe")?
        .take(131_073)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > 131_072 {
        let _ = child.kill();
    }
    let status = child.wait()?;
    read?;
    ensure!(
        bytes.len() <= 131_072,
        "integrated diff exceeds 128 KiB; split the task before review"
    );
    ensure!(status.success(), "could not read integrated Git diff");
    String::from_utf8(bytes).context("integrated text diff is not UTF-8")
}

pub fn verify_snapshot(path: &Path, expected: &str) -> Result<()> {
    ensure!(
        snapshot(path)? == expected,
        "snapshot changed; previous evidence is invalid"
    );
    Ok(())
}

pub fn create_worktree(repo: &Path, destination: &Path, commit: &str) -> Result<()> {
    ensure!(
        matches!(commit.len(), 40 | 64) && commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "full Git commit ID required"
    );
    if destination.exists() {
        verify_worktree(repo, destination)?;
        verify_snapshot(destination, commit)?;
        return Ok(());
    }
    fs::create_dir_all(destination.parent().context("worktree parent required")?)?;
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["worktree", "add", "--detach", "--"])
        .arg(destination)
        .arg(commit)
        .output()?;
    ensure!(
        output.status.success(),
        "git worktree add failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    verify_snapshot(destination, commit)
}

pub fn verify_worktree(repo: &Path, destination: &Path) -> Result<()> {
    ensure!(
        repository(destination)? == destination.canonicalize()?,
        "destination is not a worktree root"
    );
    ensure!(
        common_dir(repo)? == common_dir(destination)?,
        "existing directory belongs to another repository"
    );
    Ok(())
}

fn common_dir(repo: &Path) -> Result<PathBuf> {
    Ok(repo
        .join(git(repo, &["rev-parse", "--git-common-dir"])?)
        .canonicalize()?)
}

fn git(path: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .context("Git must be installed")?;
    ensure!(
        output.status.success(),
        "Git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().into())
}

/// Execute argv directly (no shell), drain both pipes, cap retained evidence, and bound runtime.
pub fn run_check(path: &Path, check: &CheckCommand) -> Result<CheckResult> {
    run_check_for(path, check, Duration::from_secs(check.timeout_seconds))
}

pub fn run_check_for(
    path: &Path,
    check: &CheckCommand,
    remaining: Duration,
) -> Result<CheckResult> {
    check.validate()?;
    let mut command = Command::new(&check.argv[0]);
    command
        .args(&check.argv[1..])
        .current_dir(path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ok(CheckResult {
                name: check.name.clone(),
                passed: false,
                evidence: format!("could not start {:?}: {error}", check.argv),
            });
        }
    };
    let stdout = child.stdout.take().context("missing stdout pipe")?;
    let stderr = child.stderr.take().context("missing stderr pipe")?;
    let out_reader = thread::spawn(move || bounded_read(stdout));
    let err_reader = thread::spawn(move || bounded_read(stderr));
    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= Duration::from_secs(check.timeout_seconds).min(remaining) {
            timed_out = true;
            kill_group(&mut child);
            break child.wait()?;
        }
        thread::sleep(Duration::from_millis(20));
    };
    // Do not leak test servers or descendants retaining stdout pipes after the main command exits.
    kill_group(&mut child);
    let stdout = out_reader
        .join()
        .map_err(|_| anyhow::anyhow!("stdout reader panicked"))??;
    let stderr = err_reader
        .join()
        .map_err(|_| anyhow::anyhow!("stderr reader panicked"))??;
    Ok(CheckResult {
        name: check.name.clone(),
        passed: status.success() && !timed_out,
        evidence: format!(
            "argv={:?}\nexit={:?} timed_out={timed_out}\nstdout:\n{stdout}\nstderr:\n{stderr}",
            check.argv,
            status.code()
        ),
    })
}

fn bounded_read(mut reader: impl Read) -> std::io::Result<String> {
    let mut retained = Vec::new();
    let mut buffer = [0; 4096];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let keep = count.min(8192 - retained.len());
        retained.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    let mut text = String::from_utf8_lossy(&retained).into_owned();
    if truncated {
        text.push_str("\n[output truncated at 8192 bytes]");
    }
    Ok(text)
}

fn kill_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // SAFETY: the child was spawned into its own process group; a negative PID targets
        // that group only. No borrowed memory crosses this OS call. ESRCH is harmless.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
    }
    let _ = child.kill();
}
