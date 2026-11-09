use std::{fs, path::Path, process::Command};
use synarch::{contracts::Run, dashboard, store::Store};

fn git(path: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn diff_uses_recorded_commits_not_dirty_worktree_and_escapes_content() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.invalid"]);
    git(&repo, &["config", "user.name", "Test"]);
    fs::write(repo.join("file"), "before\n").unwrap();
    fs::write(repo.join(".gitattributes"), "file diff=hostile\n").unwrap();
    let marker = dir.path().join("external-driver-ran");
    git(
        &repo,
        &[
            "config",
            "diff.hostile.command",
            &format!("touch {}", marker.display()),
        ],
    );
    git(&repo, &["add", ".gitattributes"]);
    git(&repo, &["add", "file"]);
    git(&repo, &["commit", "-qm", "base"]);
    let base = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("file"), "<script>committed</script>\n").unwrap();
    git(&repo, &["commit", "-qam", "change"]);
    let snapshot = git(&repo, &["rev-parse", "HEAD"]);
    fs::write(repo.join("file"), "uncommitted secret\n").unwrap();
    let path = dir.path().join("state.sqlite");
    let mut store = Store::open(&path).unwrap();
    let run = Run::new("run".into(), "task".into(), vec!["tests".into()], 2);
    store
        .create_configured(
            &run,
            &serde_json::json!({"baseline":base,"actor_directory":repo}),
        )
        .unwrap();
    drop(store);
    let conn = rusqlite::Connection::open(&path).unwrap();
    let mut state = serde_json::to_value(run).unwrap();
    state["snapshot"] = snapshot.into();
    conn.execute(
        "UPDATE runs SET state=?1 WHERE id='run'",
        [state.to_string()],
    )
    .unwrap();
    drop(conn);
    let html = dashboard::render_diff(&path, "run").unwrap();
    assert!(html.contains("&lt;script&gt;committed&lt;/script&gt;"));
    assert!(!html.contains("uncommitted secret"));
    assert!(html.contains("before"));
    assert!(!marker.exists(), "external diff drivers must not run");

    let conn = rusqlite::Connection::open(&path).unwrap();
    state["snapshot"] = "--output=/tmp/unsafe".into();
    conn.execute(
        "UPDATE runs SET state=?1 WHERE id='run'",
        [state.to_string()],
    )
    .unwrap();
    drop(conn);
    assert!(
        dashboard::render_diff(&path, "run")
            .unwrap()
            .contains("Diff unavailable")
    );
}

#[test]
fn diff_reports_unavailable_before_a_snapshot_exists() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let mut store = Store::open(&path).unwrap();
    let run = Run::new("run".into(), "task".into(), vec!["tests".into()], 2);
    store.create(&run).unwrap();
    drop(store);
    assert!(
        dashboard::render_diff(&path, "run")
            .unwrap()
            .contains("No recorded snapshot")
    );
    assert!(dashboard::render_diff(&path, "unknown").is_err());
}

#[test]
fn dashboard_reads_runs_without_exposing_dispatches_or_mutating_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let mut store = Store::open(&path).unwrap();
    let run = Run::new(
        "test-run".into(),
        "<script>alert(1)</script>".into(),
        vec!["tests".into()],
        2,
    );
    store.create(&run).unwrap();
    store
        .observe(
            "test-run",
            "dispatch_frozen",
            &serde_json::json!({"prompt":"SECRET_DISPATCH"}),
        )
        .unwrap();
    drop(store);
    let html = dashboard::render(&path, Some("test-run")).unwrap();
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>alert"));
    assert!(!html.contains("SECRET_DISPATCH"));
    assert!(html.contains("planning"));
    assert!(dashboard::render(&path, Some("missing")).is_err());
    assert!(!dir.path().join("state.sqlite.lock").exists());
}

#[test]
fn dashboard_refuses_missing_and_foreign_databases() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.sqlite");
    assert!(dashboard::render(&missing, None).is_err());
    assert!(!missing.exists());
    let foreign = dir.path().join("foreign.sqlite");
    rusqlite::Connection::open(&foreign).unwrap();
    assert!(dashboard::render(&foreign, None).is_err());
}
