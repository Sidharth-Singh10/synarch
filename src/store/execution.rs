use super::*;
use crate::{opencode::Assignment, runtime::*};

pub(super) fn get(connection: &Connection, id: &str) -> Result<Execution> {
    let encoded: String =
        connection.query_row("SELECT execution FROM attempts WHERE id=?1", [id], |r| {
            r.get(0)
        })?;
    Ok(serde_json::from_str(&encoded)?)
}

pub(super) fn put(connection: &Connection, execution: &Execution) -> Result<()> {
    connection.execute(
        "UPDATE attempts SET execution=?2 WHERE id=?1",
        params![execution.attempt.id, serde_json::to_string(execution)?],
    )?;
    Ok(())
}

fn insert(
    connection: &Connection,
    run: &Run,
    sequence: u32,
    dispatch: Option<Assignment>,
) -> Result<Execution> {
    let attempt = Attempt {
        id: Uuid::new_v4().to_string(),
        run_id: run.id.clone(),
        revision: run.revision,
        role: run.phase.role().context("run is terminal")?,
        session_id: format!("ses_{}", Uuid::new_v4().simple()),
        prompt_id: format!("msg_{}", Uuid::new_v4().simple()),
    };
    let mut execution = Execution::new(attempt, sequence);
    execution.dispatch = dispatch;
    connection.execute("INSERT INTO attempts(id,run_id,revision,sequence,assignment,execution) VALUES(?1,?2,?3,?4,?5,?6)",
        params![execution.attempt.id,run.id,i64::try_from(run.revision)?,sequence,
            serde_json::to_string(&execution.attempt)?,serde_json::to_string(&execution)?])?;
    record(
        connection,
        &run.id,
        "attempt_prepared",
        &serde_json::to_value(&execution.attempt)?,
    )?;
    Ok(execution)
}

impl Store {
    pub(super) fn prepare_execution(&mut self, run_id: &str) -> Result<Attempt> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run = load(&tx, run_id)?;
        ensure!(!run.paused, "run is paused");
        ensure!(run.phase.role().is_some(), "run is terminal");
        let existing: Option<String> = tx.query_row("SELECT execution FROM attempts WHERE run_id=?1 AND revision=?2 ORDER BY sequence DESC LIMIT 1",
            params![run_id,i64::try_from(run.revision)?], |r| r.get(0)).optional()?;
        let execution = match existing {
            Some(encoded) => serde_json::from_str(&encoded)?,
            None => insert(&tx, &run, 1, None)?,
        };
        tx.commit()?;
        Ok(execution.attempt)
    }

    pub fn execution(&self, id: &str) -> Result<Execution> {
        get(&self.connection, id)
    }

    pub fn attempts(&self, run_id: &str) -> Result<Vec<Execution>> {
        let mut statement = self
            .connection
            .prepare("SELECT execution FROM attempts WHERE run_id=?1 ORDER BY revision,sequence")?;
        let rows = statement.query_map([run_id], |r| r.get::<_, String>(0))?;
        rows.map(|row| Ok(serde_json::from_str(&row?)?)).collect()
    }

    pub fn freeze(&mut self, id: &str, dispatch: &Assignment) -> Result<Execution> {
        ensure!(
            dispatch.model.is_some(),
            "resolve a specific model before freezing dispatch"
        );
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut execution = get(&tx, id)?;
        ensure!(
            !execution.legacy_unfrozen,
            "original legacy dispatch is unavailable; cancel and create a new run"
        );
        ensure!(
            execution.state.executable(),
            "attempt requires explicit operator recovery"
        );
        if let Some(previous) = &execution.dispatch {
            ensure!(
                serde_json::to_value(previous)? == serde_json::to_value(dispatch)?,
                "idempotency conflict: frozen dispatch cannot change"
            );
        } else {
            ensure!(
                execution.started_ms.is_none(),
                "cannot replace an in-flight legacy dispatch"
            );
            execution.dispatch = Some(dispatch.clone());
            put(&tx, &execution)?;
            record(
                &tx,
                &execution.attempt.run_id,
                "dispatch_frozen",
                &json!({"attempt_id":id,"dispatch":dispatch}),
            )?;
        }
        tx.commit()?;
        Ok(execution)
    }

    /// Journal the launch intent and absolute deadline before any external execution.
    pub fn begin(&mut self, id: &str, now: u64) -> Result<Execution> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut execution = get(&tx, id)?;
        let run = load(&tx, &execution.attempt.run_id)?;
        ensure!(
            !run.paused && run.phase.role().is_some(),
            "run is paused or terminal"
        );
        ensure!(run.revision == execution.attempt.revision, "stale attempt");
        ensure!(
            execution.state.executable(),
            "attempt requires explicit operator recovery"
        );
        let mut control = super::controls::get(&tx, &run.id)?;
        ensure!(now < control.deadline_ms, "run_deadline_exceeded");
        if let Some(deadline) = execution.deadline_ms {
            ensure!(now < deadline, "worker_deadline_exceeded");
        }
        if execution.started_ms.is_none() {
            if execution.attempt.role != Role::Controller {
                ensure!(
                    execution.dispatch.is_some(),
                    "dispatch must be frozen before launch"
                );
                ensure!(
                    control.launches < control.limits.max_agent_launches,
                    "launch_budget_exceeded"
                );
                control.launches += 1;
            }
            execution.started_ms = Some(now);
            execution.deadline_ms = Some(
                control
                    .deadline_ms
                    .min(now.saturating_add(control.limits.worker_timeout_seconds * 1000)),
            );
            execution.stop_confirmed = execution.attempt.role == Role::Controller;
            execution.state = AttemptState::Running;
            put(&tx, &execution)?;
            super::controls::put(&tx, &run.id, &control)?;
            record(
                &tx,
                &run.id,
                "attempt_started",
                &json!({"attempt_id":id,"deadline_ms":execution.deadline_ms,"launches":control.launches}),
            )?;
        }
        tx.commit()?;
        Ok(execution)
    }

    pub fn mark_attempt(
        &mut self,
        id: &str,
        state: AttemptState,
        stopped: bool,
        code: &str,
    ) -> Result<()> {
        failure_code(code)?;
        ensure!(
            !matches!(
                state,
                AttemptState::Prepared | AttemptState::Running | AttemptState::Succeeded
            ),
            "use admission/completion methods for executable or successful states"
        );
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut execution = get(&tx, id)?;
        ensure!(
            execution.state != AttemptState::Succeeded,
            "completed attempt is immutable"
        );
        execution.state = state;
        execution.stop_confirmed = stopped;
        execution.failure_code = Some(code.into());
        put(&tx, &execution)?;
        record(
            &tx,
            &execution.attempt.run_id,
            "attempt_state_changed",
            &json!({"attempt_id":id,"state":state,"stop_confirmed":stopped,"code":code}),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn retry(&mut self, run_id: &str, reason: &str, now: u64) -> Result<Execution> {
        text(reason)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let run = load(&tx, run_id)?;
        ensure!(
            !run.paused && run.phase.role().is_some(),
            "resolve terminal escalation or resume scheduling first"
        );
        let mut execution: Execution = {
            let encoded: String = tx.query_row("SELECT execution FROM attempts WHERE run_id=?1 AND revision=?2 ORDER BY sequence DESC LIMIT 1",
                params![run_id,i64::try_from(run.revision)?], |r| r.get(0))?;
            serde_json::from_str(&encoded)?
        };
        ensure!(
            !execution.legacy_unfrozen
                && (execution.dispatch.is_some()
                    || execution.attempt.role == Role::Controller
                    || (execution.started_ms.is_none() && execution.stop_confirmed)),
            "legacy unfrozen dispatch cannot be replayed; cancel and create a new run"
        );
        let control = super::controls::get(&tx, run_id)?;
        ensure!(now < control.deadline_ms, "run_deadline_exceeded");
        let original_id = execution.attempt.id.clone();
        match execution.state {
            AttemptState::Blocked => {
                ensure!(
                    execution.deadline_ms.is_none_or(|deadline| now < deadline),
                    "worker_deadline_exceeded"
                );
                execution.state = if execution.started_ms.is_some() {
                    AttemptState::Running
                } else {
                    AttemptState::Prepared
                };
                execution.failure_code = None;
                put(&tx, &execution)?;
            }
            AttemptState::Failed | AttemptState::Interrupted | AttemptState::TimedOut => {
                ensure!(
                    execution.stop_confirmed,
                    "worker termination is unknown; reconcile or cancel before a new attempt"
                );
                ensure!(
                    execution.sequence < control.limits.max_attempts_per_stage,
                    "stage_attempt_budget_exceeded"
                );
                execution = insert(&tx, &run, execution.sequence + 1, execution.dispatch)?;
            }
            _ => anyhow::bail!("attempt is active, completed, or awaiting stop confirmation"),
        }
        record(
            &tx,
            run_id,
            "retry_requested",
            &json!({"reason":reason,"previous_attempt_id":original_id,"attempt_id":execution.attempt.id}),
        )?;
        tx.commit()?;
        Ok(execution)
    }
}
