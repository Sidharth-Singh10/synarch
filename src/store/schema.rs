use super::*;
use crate::runtime::{AttemptState, Control, Execution, Limits, now_ms};

pub(super) fn migrate(connection: &mut Connection) -> Result<()> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    ensure!(
        version <= 2,
        "database was created by a newer Synarch version"
    );
    if version == 0 {
        tx.execute_batch(
            "CREATE TABLE runs(id TEXT PRIMARY KEY, state TEXT NOT NULL, config TEXT NOT NULL);
             CREATE TABLE events(sequence INTEGER PRIMARY KEY AUTOINCREMENT, run_id TEXT NOT NULL REFERENCES runs(id),
               kind TEXT NOT NULL, payload TEXT NOT NULL, created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
             CREATE INDEX events_run ON events(run_id,sequence);"
        )?;
    }
    if version < 2 {
        tx.execute_batch(
            "CREATE TABLE attempts_v2(id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES runs(id), revision INTEGER NOT NULL,
               sequence INTEGER NOT NULL, assignment TEXT NOT NULL, execution TEXT NOT NULL, submission TEXT, result TEXT,
               UNIQUE(run_id,revision,sequence));
             CREATE TABLE controls(run_id TEXT PRIMARY KEY REFERENCES runs(id), control TEXT NOT NULL);"
        )?;
        if version == 1 {
            let rows: Vec<(String, Option<String>, Option<String>)> = {
                let mut statement =
                    tx.prepare("SELECT assignment,submission,result FROM attempts")?;
                statement
                    .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for (assignment, submission, result) in rows {
                let attempt: Attempt = serde_json::from_str(&assignment)?;
                let mut execution = Execution::new(attempt.clone(), 1);
                if result.is_some() {
                    execution.state = AttemptState::Succeeded;
                } else {
                    // A V1 unfinished assignment may already have external effects, but its
                    // original prompt was never stored. Never synthesize and replay a new one.
                    execution.state = AttemptState::Blocked;
                    execution.stop_confirmed = false;
                    execution.failure_code = Some("legacy_unfrozen_dispatch".into());
                    execution.legacy_unfrozen = true;
                }
                tx.execute("INSERT INTO attempts_v2(id,run_id,revision,sequence,assignment,execution,submission,result) VALUES(?1,?2,?3,1,?4,?5,?6,?7)",
                    params![attempt.id,attempt.run_id,i64::try_from(attempt.revision)?,assignment,serde_json::to_string(&execution)?,submission,result])?;
            }
            tx.execute_batch("DROP TABLE attempts;")?;
            let runs: Vec<String> = {
                let mut statement = tx.prepare("SELECT id FROM runs")?;
                statement
                    .query_map([], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for id in runs {
                let control = Control::new(Limits::default(), now_ms())?;
                tx.execute(
                    "INSERT INTO controls(run_id,control) VALUES(?1,?2)",
                    params![id, serde_json::to_string(&control)?],
                )?;
                record(
                    &tx,
                    &id,
                    "schema_migrated",
                    &json!({"from":1,"to":2,"unfinished_dispatches":"blocked_until_operator_reconciliation"}),
                )?;
            }
        }
        tx.execute_batch("ALTER TABLE attempts_v2 RENAME TO attempts; PRAGMA application_id=0x53594e41; PRAGMA user_version=2;")?;
    }
    tx.commit()?;
    Ok(())
}
