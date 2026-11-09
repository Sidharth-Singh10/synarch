use std::{fs, path::Path, process::Command};
use synarch::workspace::*;

fn git(path: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q"]);
    git(
        dir.path(),
        &["config", "user.email", "test@example.invalid"],
    );
    git(dir.path(), &["config", "user.name", "Synarch tests"]);
    fs::write(dir.path().join("source.txt"), "baseline\n").unwrap();
    git(dir.path(), &["add", "source.txt"]);
    git(dir.path(), &["commit", "-qm", "test: create fixture"]);
    dir
}

#[test]
fn captures_only_clean_committed_snapshots_including_untracked_files() {
    let repo = fixture();
    assert_eq!(
        snapshot(repo.path()).unwrap(),
        git(repo.path(), &["rev-parse", "HEAD"])
    );
    fs::write(repo.path().join("untracked.txt"), "extra").unwrap();
    assert!(snapshot(repo.path()).is_err());
}

#[test]
fn creates_detached_verification_worktree_without_touching_original() {
    let repo = fixture();
    let root = tempfile::tempdir().unwrap();
    let head = snapshot(repo.path()).unwrap();
    let path = root.path().join("qa");
    create_worktree(repo.path(), &path, &head).unwrap();
    assert_eq!(snapshot(&path).unwrap(), head);
    assert_eq!(snapshot(repo.path()).unwrap(), head);
    // Crash recovery verifies ownership and reuses exactly the same worktree.
    create_worktree(repo.path(), &path, &head).unwrap();
    fs::write(path.join("source.txt"), "dirty").unwrap();
    assert!(create_worktree(repo.path(), &path, &head).is_err());
}

#[test]
fn never_adopts_an_unrelated_existing_directory() {
    let repo = fixture();
    let other = fixture();
    assert!(create_worktree(repo.path(), other.path(), &snapshot(repo.path()).unwrap()).is_err());
}

#[test]
fn real_checks_capture_success_failure_and_argument_boundaries() {
    let repo = fixture();
    let result = run_check(
        repo.path(),
        &CheckCommand {
            name: "literal".into(),
            argv: vec!["printf".into(), "%s".into(), "$(touch injected)".into()],
            timeout_seconds: 3,
        },
    )
    .unwrap();
    assert!(result.passed);
    assert!(result.evidence.contains("$(touch injected)"));
    assert!(!repo.path().join("injected").exists());
    let result = run_check(
        repo.path(),
        &CheckCommand {
            name: "fail".into(),
            argv: vec!["false".into()],
            timeout_seconds: 3,
        },
    )
    .unwrap();
    assert!(!result.passed);
    assert!(result.evidence.contains("exit=Some(1)"));
}

#[test]
fn timed_out_checks_fail_and_output_is_bounded() {
    let repo = fixture();
    let started = std::time::Instant::now();
    let result = run_check(
        repo.path(),
        &CheckCommand {
            name: "timeout".into(),
            argv: vec!["sleep".into(), "10".into()],
            timeout_seconds: 1,
        },
    )
    .unwrap();
    assert!(!result.passed);
    assert!(result.evidence.contains("timed_out=true"));
    assert!(started.elapsed().as_secs() < 5);
    let result = run_check(
        repo.path(),
        &CheckCommand {
            name: "output".into(),
            argv: vec![
                "head".into(),
                "-c".into(),
                "1000000".into(),
                "/dev/zero".into(),
            ],
            timeout_seconds: 3,
        },
    )
    .unwrap();
    assert!(result.passed);
    assert!(result.evidence.len() < 65_536);
    assert!(result.evidence.contains("truncated"));
}

#[test]
fn missing_executable_is_a_failed_check_not_a_pass_or_omission() {
    let repo = fixture();
    let result = run_check(
        repo.path(),
        &CheckCommand {
            name: "missing".into(),
            argv: vec!["synarch-nonexistent-executable".into()],
            timeout_seconds: 3,
        },
    )
    .unwrap();
    assert!(!result.passed);
    assert!(result.evidence.contains("could not start"));
}
