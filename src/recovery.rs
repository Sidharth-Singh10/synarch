//! Operator actions never supply technical acceptance or manufacture a QA pass.
use crate::{
    contracts::*,
    opencode::{AgentBackend, WorkerOutcome, WorkerState},
    runtime::*,
    store::Store,
};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::path::Path;

fn stop_pending(store: &mut Store, run_id: &str, backend: Option<&dyn AgentBackend>) -> Result<()> {
    let control = store.control(run_id)?;
    for execution in store
        .attempts(run_id)?
        .into_iter()
        .filter(|e| !e.stop_confirmed)
    {
        let backend = backend.context("server connection required to confirm worker termination; stop intent remains journaled")?;
        check_endpoint(&execution, backend)?;
        let state = backend.interrupt(&execution.attempt)?;
        store.observe(
            run_id,
            "worker_stop_observed",
            &json!({"attempt_id":execution.attempt.id,"worker":state}),
        )?;
        ensure!(
            state.stopped(),
            "worker is still running or has queued input; cancellation is not confirmed"
        );
        store.mark_attempt(
            &execution.attempt.id,
            if control.cancel_requested {
                AttemptState::Interrupted
            } else {
                AttemptState::TimedOut
            },
            true,
            control.halt_reason.as_deref().unwrap_or("operator_stop"),
        )?;
    }
    Ok(())
}

pub(crate) fn expire(
    store: &mut Store,
    run_id: &str,
    backend: &dyn AgentBackend,
    code: &str,
) -> Result<()> {
    store.halt(run_id, code, false)?;
    stop_pending(store, run_id, Some(backend))
}

pub fn cancel(
    database: &Path,
    run_id: &str,
    reason: &str,
    backend: Option<&dyn AgentBackend>,
) -> Result<Run> {
    text(reason)?;
    let _lock = Store::lock(database)?;
    let mut store = Store::open(database)?;
    store.halt(run_id, "operator_cancelled", true)?;
    store.observe(run_id, "cancel_requested", &json!({"reason":reason}))?;
    stop_pending(&mut store, run_id, backend)?;
    store.load(run_id)
}

pub fn retry(
    database: &Path,
    run_id: &str,
    reason: &str,
    backend: Option<&dyn AgentBackend>,
) -> Result<Execution> {
    text(reason)?;
    let _lock = Store::lock(database)?;
    let mut store = Store::open(database)?;
    let run = store.load(run_id)?;
    ensure!(
        run.phase.role().is_some(),
        "resolve escalation first; cancelled runs cannot be retried"
    );
    let execution = store
        .attempts(run_id)?
        .into_iter()
        .rev()
        .find(|e| e.attempt.revision == run.revision)
        .context("no attempt to retry")?;
    let control = store.control(run_id)?;
    let now = now_ms();
    let exhausted = if now >= control.deadline_ms {
        Some("run_deadline_exceeded")
    } else if execution
        .deadline_ms
        .is_some_and(|deadline| now >= deadline)
    {
        Some("worker_deadline_exceeded")
    } else {
        None
    };
    if let Some(code) = exhausted {
        store.halt(run_id, code, false)?;
        stop_pending(&mut store, run_id, backend)?;
        anyhow::bail!("{code}; resolve escalation before retrying");
    }
    if !execution.stop_confirmed {
        let backend =
            backend.context("server connection required to reconcile the existing worker")?;
        check_endpoint(&execution, backend)?;
        let state = backend.inspect(&execution.attempt)?;
        store.observe(
            run_id,
            "worker_reconciled",
            &json!({"attempt_id":execution.attempt.id,"worker":state}),
        )?;
        match state {
            WorkerState::Stopped {
                outcome: WorkerOutcome::Failed,
            } => store.mark_attempt(
                &execution.attempt.id,
                AttemptState::Failed,
                true,
                "worker_failed",
            )?,
            WorkerState::Stopped {
                outcome: WorkerOutcome::Interrupted,
            } => store.mark_attempt(
                &execution.attempt.id,
                AttemptState::Interrupted,
                true,
                "worker_interrupted",
            )?,
            WorkerState::Stopped { .. } | WorkerState::Missing => store.mark_attempt(
                &execution.attempt.id,
                AttemptState::Blocked,
                true,
                "worker_stopped",
            )?,
            WorkerState::Running => (), // Explicitly resume observing the SAME assignment, not a new launch.
        }
    }
    match store.retry(run_id, reason, now_ms()) {
        Err(error) if error.to_string() == "stage_attempt_budget_exceeded" => {
            store.halt(run_id, "stage_attempt_budget_exceeded", false)?;
            Err(error)
        }
        result => result,
    }
}

fn check_endpoint(execution: &Execution, backend: &dyn AgentBackend) -> Result<()> {
    if let Some(expected) = execution
        .dispatch
        .as_ref()
        .and_then(|d| d.endpoint.as_deref())
    {
        ensure!(
            backend.endpoint() == Some(expected),
            "server endpoint differs from frozen dispatch; worker termination is not confirmed"
        );
    }
    Ok(())
}

pub fn resolve(
    database: &Path,
    run_id: &str,
    reason: &str,
    additional_seconds: u64,
    additional_launches: u32,
    additional_iterations: u32,
    backend: Option<&dyn AgentBackend>,
) -> Result<Run> {
    text(reason)?;
    ensure!(
        additional_seconds <= 604800 && additional_launches <= 1000 && additional_iterations <= 100,
        "requested resource extension exceeds supported bounds"
    );
    let _lock = Store::lock(database)?;
    let mut store = Store::open(database)?;
    ensure!(
        store.load(run_id)?.phase == Phase::Escalated,
        "run is not escalated"
    );
    stop_pending(&mut store, run_id, backend)?;
    store.resolve(
        run_id,
        reason,
        additional_seconds,
        additional_launches,
        additional_iterations,
        now_ms(),
    )
}
