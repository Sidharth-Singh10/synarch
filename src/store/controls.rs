use super::*;
use crate::runtime::{AttemptState, Control, failure_code};

pub(super) fn get(connection: &Connection, run_id: &str) -> Result<Control> {
    let encoded: String = connection.query_row(
        "SELECT control FROM controls WHERE run_id=?1",
        [run_id],
        |r| r.get(0),
    )?;
    Ok(serde_json::from_str(&encoded)?)
}

pub(super) fn put(connection: &Connection, run_id: &str, control: &Control) -> Result<()> {
    connection.execute(
        "UPDATE controls SET control=?2 WHERE run_id=?1",
        params![run_id, serde_json::to_string(control)?],
    )?;
    Ok(())
}

impl Store {
    pub fn control(&self, run_id: &str) -> Result<Control> {
        get(&self.connection, run_id)
    }

    /// Persist the stop intent before any network calls. No new work can be admitted.
    pub fn halt(&mut self, run_id: &str, code: &str, cancelled: bool) -> Result<()> {
        failure_code(code)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut run = load(&tx, run_id)?;
        ensure!(
            run.phase != Phase::Ready,
            "completed runs cannot be cancelled or escalated operationally"
        );
        let mut control = get(&tx, run_id)?;
        if control.halted_phase.is_none() && run.phase.role().is_some() {
            control.halted_phase = Some(run.phase);
        }
        control.cancel_requested |= cancelled;
        control.halt_reason = Some(code.into());
        run.phase = if control.cancel_requested {
            Phase::Cancelled
        } else {
            Phase::Escalated
        };
        run.qa_validated = false;
        tx.execute(
            "UPDATE runs SET state=?2 WHERE id=?1",
            params![run_id, serde_json::to_string(&run)?],
        )?;
        put(&tx, run_id, &control)?;
        let executions: Vec<String> = {
            let mut statement = tx.prepare("SELECT execution FROM attempts WHERE run_id=?1")?;
            statement
                .query_map([run_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for encoded in executions {
            let mut execution: crate::runtime::Execution = serde_json::from_str(&encoded)?;
            if execution.state == AttemptState::Succeeded {
                continue;
            }
            if !execution.stop_confirmed {
                execution.state = AttemptState::Cancelling;
            } else if execution.state.executable() {
                execution.state = if cancelled {
                    AttemptState::Interrupted
                } else {
                    AttemptState::TimedOut
                };
            }
            execution.failure_code = Some(code.into());
            super::execution::put(&tx, &execution)?;
        }
        record(
            &tx,
            run_id,
            "stop_requested",
            &json!({"code":code,"cancelled":control.cancel_requested,"resume_phase":control.halted_phase}),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn resolve(
        &mut self,
        run_id: &str,
        reason: &str,
        additional_seconds: u64,
        additional_launches: u32,
        additional_iterations: u32,
        now: u64,
    ) -> Result<Run> {
        text(reason)?;
        ensure!(
            additional_seconds <= 604800,
            "budget extension is limited to seven days per resolution"
        );
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut run = load(&tx, run_id)?;
        ensure!(
            run.phase == Phase::Escalated,
            "only escalated runs can be resolved; cancellation is terminal"
        );
        let mut control = get(&tx, run_id)?;
        ensure!(
            !control.cancel_requested,
            "cancelled runs cannot be resumed"
        );
        let unsafe_count: i64 = tx.query_row("SELECT count(*) FROM attempts WHERE run_id=?1 AND json_extract(execution,'$.stop_confirmed')=0", [run_id], |r| r.get(0))?;
        ensure!(
            unsafe_count == 0,
            "worker termination is not confirmed; reconcile cancellation first"
        );
        let phase = control
            .halted_phase
            .context("missing authorized resume phase")?;
        ensure!(
            phase.role().is_some(),
            "cannot resolve directly into acceptance or release readiness"
        );
        if additional_seconds > 0 {
            control.deadline_ms = control
                .deadline_ms
                .max(now)
                .checked_add(additional_seconds * 1000)
                .context("deadline overflow")?;
        }
        control.limits.max_agent_launches = control
            .limits
            .max_agent_launches
            .checked_add(additional_launches)
            .context("launch budget overflow")?;
        control.limits.validate()?;
        run.max_iterations = run
            .max_iterations
            .checked_add(additional_iterations)
            .context("iteration budget overflow")?;
        ensure!(
            run.max_iterations <= 100,
            "iteration budget cannot exceed 100"
        );
        ensure!(
            now < control.deadline_ms,
            "extend the expired run deadline explicitly"
        );
        ensure!(
            control.launches < control.limits.max_agent_launches,
            "extend the exhausted launch budget explicitly"
        );
        if control.halt_reason.as_deref() == Some("jury_escalation")
            && matches!(phase, Phase::Implementing | Phase::Experiment | Phase::Qa)
        {
            ensure!(
                run.iteration < run.max_iterations,
                "extend the exhausted iteration budget explicitly"
            );
            run.iteration += 1;
        }
        run.operator_context = if run.operator_context.is_empty() {
            reason.into()
        } else {
            format!("{}\n{reason}", run.operator_context)
        };
        text(&run.operator_context)?;
        run.phase = phase;
        run.paused = false;
        run.revision += 1;
        control.halted_phase = None;
        control.halt_reason = None;
        tx.execute(
            "UPDATE runs SET state=?2 WHERE id=?1",
            params![run_id, serde_json::to_string(&run)?],
        )?;
        put(&tx, run_id, &control)?;
        record(
            &tx,
            run_id,
            "escalation_resolved",
            &json!({"reason":reason,"additional_seconds":additional_seconds,
            "additional_launches":additional_launches,"additional_iterations":additional_iterations,"resume_phase":phase,"revision":run.revision}),
        )?;
        tx.commit()?;
        Ok(run)
    }
}
