//! SQLite is the authority; the live OpenCode event stream is not a ledger.
use crate::{contracts::*, engine};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    path::Path,
    time::Duration,
};
use uuid::Uuid;

mod controls;
mod execution;
mod schema;

pub struct Store {
    connection: Connection,
}

pub struct ControllerLock {
    file: File,
}

impl Drop for ControllerLock {
    fn drop(&mut self) {
        // Explicitly unlock the shared file description: close alone can leave ownership
        // temporarily held by a descriptor inherited during another thread's fork.
        let _ = self.file.unlock();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub id: String,
    pub run_id: String,
    pub revision: u64,
    pub role: Role,
    pub session_id: String,
    pub prompt_id: String,
}

#[derive(Debug, Serialize)]
pub struct Event {
    pub sequence: i64,
    pub kind: String,
    pub payload: Value,
    pub created_at: String,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Precreate privately; SQLite otherwise uses the process umask (often 0644).
        drop(options.open(path)?);
        Self::initialize(Connection::open(path)?)
    }

    pub fn open_memory() -> Result<Self> {
        Self::initialize(Connection::open_in_memory()?)
    }

    fn initialize(mut connection: Connection) -> Result<Self> {
        connection.busy_timeout(Duration::from_secs(5))?;
        let application: i64 = connection.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        let objects: i64 =
            connection.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
        ensure!(
            application == 0x5359_4e41 || (application == 0 && version == 0 && objects == 0),
            "refusing to modify a database not owned by Synarch"
        );
        ensure!(
            version <= 2,
            "database was created by a newer Synarch version"
        );
        connection.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
        )?;
        schema::migrate(&mut connection)?;
        Ok(Self { connection })
    }

    /// Held only by an execution step. OS releases it after a controller crash.
    /// This is deliberately stronger/simpler than an expiring lease for the single-host MVP.
    pub fn lock(path: &Path) -> Result<ControllerLock> {
        let absolute = if path.exists() {
            path.canonicalize()?
        } else {
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            parent
                .canonicalize()?
                .join(path.file_name().context("database filename required")?)
        };
        let mut name = absolute.as_os_str().to_os_string();
        name.push(".lock");
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(name)?;
        file.try_lock()
            .context("another controller owns this database; retry after its step finishes")?;
        Ok(ControllerLock { file })
    }

    pub fn create(&mut self, run: &Run) -> Result<()> {
        self.create_configured(run, &json!({}))
    }

    pub fn create_configured(&mut self, run: &Run, config: &Value) -> Result<()> {
        run.validate()?;
        ensure!(
            run.phase == Phase::Planning && run.revision == 0,
            "new runs must start at planning revision zero"
        );
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT INTO runs(id,state,config) VALUES(?1,?2,?3)",
            params![run.id, serde_json::to_string(run)?, config.to_string()],
        )?;
        let limits = config
            .pointer("/config/limits")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        let control = crate::runtime::Control::new(limits, crate::runtime::now_ms())?;
        tx.execute(
            "INSERT INTO controls(run_id,control) VALUES(?1,?2)",
            params![run.id, serde_json::to_string(&control)?],
        )?;
        record(&tx, &run.id, "run_created", &serde_json::to_value(run)?)?;
        tx.commit()?;
        Ok(())
    }

    pub fn load(&self, id: &str) -> Result<Run> {
        load(&self.connection, id)
    }

    pub fn config(&self, id: &str) -> Result<Value> {
        let value: String =
            self.connection
                .query_row("SELECT config FROM runs WHERE id=?1", [id], |r| r.get(0))?;
        Ok(serde_json::from_str(&value)?)
    }

    pub fn prepare(&mut self, run_id: &str) -> Result<Attempt> {
        self.prepare_execution(run_id)
    }

    pub fn complete(
        &mut self,
        attempt_id: &str,
        snapshot: Option<&str>,
        report: Report,
    ) -> Result<Run> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (assignment, submission, result): (String, Option<String>, Option<String>) = tx
            .query_row(
                "SELECT assignment,submission,result FROM attempts WHERE id=?1",
                [attempt_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
        let attempt: Attempt = serde_json::from_str(&assignment)?;
        let input = json!({"snapshot":snapshot,"report":report});
        if let Some(previous) = submission {
            ensure!(
                serde_json::from_str::<Value>(&previous)? == input,
                "idempotency conflict: changed report or snapshot"
            );
            return Ok(serde_json::from_str(
                &result.context("missing persisted transition result")?,
            )?);
        }
        let run = load(&tx, &attempt.run_id)?;
        ensure!(run.revision == attempt.revision, "stale attempt revision");
        let mut execution = execution::get(&tx, attempt_id)?;
        ensure!(
            execution.state.executable(),
            "attempt requires explicit operator recovery"
        );
        if let Some(deadline) = execution.deadline_ms {
            let now = crate::runtime::now_ms();
            ensure!(
                now < deadline && now < controls::get(&tx, &run.id)?.deadline_ms,
                "execution_deadline_exceeded"
            );
        }
        let next = engine::apply(&run, attempt.role, snapshot, report.clone())?;
        execution.state = crate::runtime::AttemptState::Succeeded;
        execution.stop_confirmed = true;
        execution::put(&tx, &execution)?;
        if next.phase == Phase::Escalated {
            let mut control = controls::get(&tx, &run.id)?;
            control.halted_phase = Some(match report {
                Report::Decision {
                    verdict: Verdict::Rework,
                    ..
                } => Phase::Implementing,
                Report::Decision {
                    verdict: Verdict::Experiment,
                    ..
                } => Phase::Experiment,
                Report::Decision {
                    verdict: Verdict::RetryQa,
                    ..
                } => Phase::Qa,
                _ => run.phase,
            });
            control.halt_reason = Some("jury_escalation".into());
            controls::put(&tx, &run.id, &control)?;
        }
        let encoded = serde_json::to_string(&next)?;
        tx.execute(
            "UPDATE runs SET state=?2 WHERE id=?1",
            params![run.id, encoded],
        )?;
        tx.execute(
            "UPDATE attempts SET submission=?2,result=?3 WHERE id=?1",
            params![attempt_id, input.to_string(), encoded],
        )?;
        record(
            &tx,
            &run.id,
            "transition",
            &json!({"attempt_id":attempt_id,"role":attempt.role,"from":run.phase,"to":next.phase,"submission":input}),
        )?;
        tx.commit()?;
        Ok(next)
    }

    pub fn set_paused(&mut self, run_id: &str, paused: bool) -> Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut run = load(&tx, run_id)?;
        run.paused = paused;
        tx.execute(
            "UPDATE runs SET state=?2 WHERE id=?1",
            params![run_id, serde_json::to_string(&run)?],
        )?;
        record(
            &tx,
            run_id,
            if paused { "paused" } else { "resumed" },
            &json!({}),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn events(&self, run_id: &str) -> Result<Vec<Event>> {
        let mut statement = self.connection.prepare(
            "SELECT sequence,kind,payload,created_at FROM events WHERE run_id=?1 ORDER BY sequence",
        )?;
        let rows = statement.query_map([run_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (sequence, kind, payload, created_at) = row?;
            Ok(Event {
                sequence,
                kind,
                payload: serde_json::from_str(&payload)?,
                created_at,
            })
        })
        .collect()
    }

    pub fn observe(&self, run_id: &str, kind: &str, payload: &Value) -> Result<()> {
        record(&self.connection, run_id, kind, payload)
    }
}

fn load(connection: &Connection, id: &str) -> Result<Run> {
    let value: String = connection
        .query_row("SELECT state FROM runs WHERE id=?1", [id], |r| r.get(0))
        .context("run not found")?;
    Ok(serde_json::from_str(&value)?)
}

fn record(connection: &Connection, run_id: &str, kind: &str, payload: &Value) -> Result<()> {
    connection.execute(
        "INSERT INTO events(run_id,kind,payload) VALUES(?1,?2,?3)",
        params![run_id, kind, payload.to_string()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn releasing_controller_ownership_does_not_wait_for_a_duplicated_descriptor() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("state.sqlite");
        let lock = Store::lock(&db).unwrap();
        // Models the shared descriptor inherited by another thread's fork before exec.
        let _inherited = lock.file.try_clone().unwrap();
        drop(lock);
        assert!(Store::lock(&db).is_ok());
    }
}
