use synarch::contracts::*;
use synarch::store::Store;

fn initial() -> Run {
    Run::new(
        "run-1".into(),
        "Fix shutdown".into(),
        vec!["test".into()],
        3,
    )
}

fn plan() -> Report {
    Report::Plan {
        acceptance: vec!["No hang".into()],
        instructions: "Fix shutdown".into(),
    }
}

#[test]
fn interrupted_dispatch_reuses_attempt_session_and_prompt_ids() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    store.create(&initial()).unwrap();
    let first = store.prepare("run-1").unwrap();
    drop(store);
    let mut reopened = Store::open(&db).unwrap();
    let recovered = reopened.prepare("run-1").unwrap();
    assert_eq!(first.id, recovered.id);
    assert_eq!(first.session_id, recovered.session_id);
    assert_eq!(first.prompt_id, recovered.prompt_id);
    assert_eq!(reopened.load("run-1").unwrap().phase, Phase::Planning);
}

#[test]
fn duplicate_report_replays_but_changed_payload_is_rejected() {
    let mut store = Store::open_memory().unwrap();
    store.create(&initial()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    let first = store.complete(&attempt.id, None, plan()).unwrap();
    let replay = store.complete(&attempt.id, None, plan()).unwrap();
    assert_eq!(first.revision, replay.revision);
    assert_eq!(store.events("run-1").unwrap().len(), 3);
    assert!(
        store
            .complete(
                &attempt.id,
                None,
                Report::Plan {
                    acceptance: vec!["Other".into()],
                    instructions: "Different".into(),
                }
            )
            .is_err()
    );
}

#[test]
fn concurrent_connections_cannot_reserve_two_attempts_for_one_revision() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    let mut first = Store::open(&db).unwrap();
    let mut second = Store::open(&db).unwrap();
    first.create(&initial()).unwrap();
    assert_eq!(
        first.prepare("run-1").unwrap().id,
        second.prepare("run-1").unwrap().id
    );
}

#[test]
fn failed_transition_rolls_back_state_report_and_event() {
    let mut store = Store::open_memory().unwrap();
    store.create(&initial()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    assert!(
        store
            .complete(
                &attempt.id,
                None,
                Report::Implementation {
                    summary: "Wrong role".into()
                }
            )
            .is_err()
    );
    assert_eq!(store.load("run-1").unwrap().revision, 0);
    assert_eq!(store.events("run-1").unwrap().len(), 2);
    store.complete(&attempt.id, None, plan()).unwrap();
}

#[test]
fn pause_blocks_dispatch_and_completion_without_losing_assignment() {
    let mut store = Store::open_memory().unwrap();
    store.create(&initial()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    store.set_paused("run-1", true).unwrap();
    assert!(store.prepare("run-1").is_err());
    assert!(store.complete(&attempt.id, None, plan()).is_err());
    store.set_paused("run-1", false).unwrap();
    assert_eq!(store.prepare("run-1").unwrap().id, attempt.id);
    store.complete(&attempt.id, None, plan()).unwrap();
}

#[test]
fn a_second_controller_cannot_hold_the_same_database_lock() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    let _first = Store::lock(&db).unwrap();
    assert!(Store::lock(&db).is_err());
}

#[test]
fn refuses_to_initialize_an_unrelated_sqlite_database() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("other.sqlite");
    let other = rusqlite::Connection::open(&db).unwrap();
    other
        .execute("CREATE TABLE user_data(value TEXT)", [])
        .unwrap();
    assert!(Store::open(&db).is_err());
    let tables: i64 = other
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tables, 1);
}

#[cfg(unix)]
#[test]
fn newly_created_state_is_not_world_or_group_readable() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    let _store = Store::open(&db).unwrap();
    assert_eq!(
        std::fs::metadata(db).unwrap().permissions().mode() & 0o077,
        0
    );
}
