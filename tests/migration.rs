use rusqlite::{Connection, params};
use serde_json::json;
use synarch::{
    contracts::*,
    runtime::{AttemptState, now_ms},
    store::{Attempt, Store},
};

fn legacy(path: &std::path::Path, corrupt: bool) {
    let connection = Connection::open(path).unwrap();
    connection.execute_batch(
        "CREATE TABLE runs(id TEXT PRIMARY KEY,state TEXT NOT NULL,config TEXT NOT NULL);
         CREATE TABLE attempts(id TEXT PRIMARY KEY,run_id TEXT NOT NULL REFERENCES runs(id),revision INTEGER NOT NULL,
           assignment TEXT NOT NULL,submission TEXT,result TEXT,UNIQUE(run_id,revision));
         CREATE TABLE events(sequence INTEGER PRIMARY KEY AUTOINCREMENT,run_id TEXT NOT NULL REFERENCES runs(id),
           kind TEXT NOT NULL,payload TEXT NOT NULL,created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
         PRAGMA application_id=0x53594e41; PRAGMA user_version=1;"
    ).unwrap();
    let mut run = Run::new(
        "legacy-run".into(),
        "Fix shutdown".into(),
        vec!["test".into()],
        3,
    );
    run.phase = Phase::Implementing;
    run.revision = 1;
    connection
        .execute(
            "INSERT INTO runs VALUES(?1,?2,'{}')",
            params![run.id, serde_json::to_string(&run).unwrap()],
        )
        .unwrap();
    for (id, revision, role, result) in [
        (
            "old-completed",
            0,
            Role::Jury,
            Some(serde_json::to_string(&run).unwrap()),
        ),
        ("old-pending", 1, Role::Actor, None),
    ] {
        let attempt = Attempt {
            id: id.into(),
            run_id: run.id.clone(),
            revision,
            role,
            session_id: format!("ses_{id}"),
            prompt_id: format!("msg_{id}"),
        };
        let encoded = if corrupt && result.is_none() {
            "invalid JSON".into()
        } else {
            serde_json::to_string(&attempt).unwrap()
        };
        connection
            .execute(
                "INSERT INTO attempts VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    id,
                    run.id,
                    revision as i64,
                    encoded,
                    result
                        .as_ref()
                        .map(|_| json!({"old":"evidence"}).to_string()),
                    result
                ],
            )
            .unwrap();
    }
    connection.execute("INSERT INTO events(run_id,kind,payload) VALUES(?1,'original_evidence','{\"preserve\":true}')", [&run.id]).unwrap();
}

#[test]
fn migrates_v1_without_losing_evidence_or_recreating_an_in_flight_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.sqlite");
    legacy(&db, false);
    let mut store = Store::open(&db).unwrap();
    assert_eq!(store.load("legacy-run").unwrap().revision, 1);
    let attempts = store.attempts("legacy-run").unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].state, AttemptState::Succeeded);
    assert_eq!(attempts[1].state, AttemptState::Blocked);
    assert!(attempts[1].legacy_unfrozen && !attempts[1].stop_confirmed);
    assert!(attempts[1].dispatch.is_none());
    assert!(
        store
            .retry("legacy-run", "Retry legacy worker", now_ms())
            .is_err()
    );
    // Reconciliation must not erase the fact that the original dispatch was never frozen.
    store
        .mark_attempt("old-pending", AttemptState::Failed, true, "worker_failed")
        .unwrap();
    assert!(
        store
            .retry("legacy-run", "Still unsafe to recreate", now_ms())
            .is_err()
    );
    let events = store.events("legacy-run").unwrap();
    assert_eq!(events[0].payload["preserve"], true);
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "schema_migrated")
            .count(),
        1
    );
    drop(store);
    let reopened = Store::open(&db).unwrap();
    assert_eq!(reopened.events("legacy-run").unwrap().len(), events.len());
}

#[test]
fn malformed_legacy_data_rolls_back_the_whole_migration() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.sqlite");
    legacy(&db, true);
    assert!(Store::open(&db).is_err());
    let connection = Connection::open(&db).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM attempts", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name IN ('controls','attempts_v2')",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM events", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}
