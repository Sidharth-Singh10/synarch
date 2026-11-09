use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn git(path: &Path, args: &[&str]) {
    assert!(
        Command::new("git")
            .current_dir(path)
            .args(args)
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[test]
fn cli_creates_inspects_pauses_and_resumes_a_persisted_run_without_a_server() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.invalid"]);
    git(&repo, &["config", "user.name", "Tests"]);
    fs::write(repo.join("source.txt"), "baseline").unwrap();
    git(&repo, &["add", "source.txt"]);
    git(&repo, &["commit", "-qm", "test: create fixture"]);
    let config_path = dir.path().join("workflow.json");
    fs::write(
        &config_path,
        json!({
            "repository":"repo", "workspace_root":"worktrees", "max_iterations":3,
            "checks":[{"name":"test","argv":["true"],"timeout_seconds":1}],
            "agents":{
                "jury":{"agent":"synarch-jury"},"actor":{"agent":"synarch-actor"},
                "critic":{"agent":"synarch-critic"},"qa":{"agent":"synarch-qa"}
            }
        })
        .to_string(),
    )
    .unwrap();
    let db = dir.path().join("state.sqlite");
    let invoke = |args: &[&str]| {
        let result = Command::new(env!("CARGO_BIN_EXE_synarch"))
            .arg("--db")
            .arg(&db)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice::<Value>(&result.stdout).unwrap()
    };
    let created = invoke(&[
        "create",
        "--config",
        config_path.to_str().unwrap(),
        "--request",
        "Fix source",
    ]);
    let id = created["id"].as_str().unwrap();
    assert_eq!(invoke(&["status", id])["phase"], "planning");
    assert_eq!(invoke(&["pause", id])["paused"], true);
    assert_eq!(invoke(&["resume", id])["paused"], false);
    let events = Command::new(env!("CARGO_BIN_EXE_synarch"))
        .arg("--db")
        .arg(&db)
        .args(["events", id])
        .output()
        .unwrap();
    assert!(events.status.success());
    let lines = String::from_utf8(events.stdout).unwrap();
    assert_eq!(lines.lines().count(), 3);
    assert!(lines.contains("run_created") && lines.contains("paused") && lines.contains("resumed"));
    assert!(
        !dir.path().join("worktrees").join(id).exists(),
        "create must not launch work or create a checkout"
    );
    assert_eq!(
        invoke(&["cancel", id, "--reason", "Stop this undispatched fixture"])["phase"],
        "cancelled"
    );
    let retry = Command::new(env!("CARGO_BIN_EXE_synarch"))
        .arg("--db")
        .arg(&db)
        .args(["retry", id, "--reason", "Cannot resume cancelled work"])
        .output()
        .unwrap();
    assert!(!retry.status.success());
}
