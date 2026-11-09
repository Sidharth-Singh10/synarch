use serde_json::json;
use synarch::{
    contracts::*,
    opencode::{Assignment, Model},
    runtime::*,
    store::Store,
};

fn run() -> Run {
    Run::new(
        "run-1".into(),
        "Fix shutdown".into(),
        vec!["test".into()],
        3,
    )
}

fn assignment() -> Assignment {
    Assignment {
        directory: "/workspace".into(),
        agent: "jury".into(),
        model: Some(Model {
            provider_id: "test".into(),
            id: "model-a".into(),
            variant: None,
        }),
        prompt: "Original prompt".into(),
        permissions: vec![json!({"action":"edit","resource":"*","effect":"deny"})],
        endpoint: None,
    }
}

#[test]
fn dispatch_is_frozen_before_execution_and_survives_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let mut store = Store::open(&path).unwrap();
    store.create(&run()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    store.freeze(&attempt.id, &assignment()).unwrap();
    drop(store);
    let mut reopened = Store::open(&path).unwrap();
    let record = reopened.execution(&attempt.id).unwrap();
    assert_eq!(record.dispatch.unwrap().prompt, "Original prompt");
    let mut changed = assignment();
    changed.prompt = "New code generated a different prompt".into();
    assert!(reopened.freeze(&attempt.id, &changed).is_err());
    changed = assignment();
    changed.model.as_mut().unwrap().id = "model-b".into();
    assert!(reopened.freeze(&attempt.id, &changed).is_err());
    changed = assignment();
    changed.permissions.clear();
    assert!(reopened.freeze(&attempt.id, &changed).is_err());
}

#[test]
fn uncertain_failure_retries_the_same_dispatch_without_restarting_deadline() {
    let mut store = Store::open_memory().unwrap();
    store.create(&run()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    store.freeze(&attempt.id, &assignment()).unwrap();
    let now = now_ms();
    let original = store.begin(&attempt.id, now).unwrap();
    store
        .mark_attempt(
            &attempt.id,
            AttemptState::Blocked,
            false,
            "transport_unknown",
        )
        .unwrap();
    let retried = store.retry("run-1", "Network recovered", now + 10).unwrap();
    assert_eq!(retried.attempt.id, attempt.id);
    assert_eq!(retried.deadline_ms, original.deadline_ms);
    assert_eq!(store.control("run-1").unwrap().launches, 1);
}

#[test]
fn confirmed_worker_failure_can_create_a_new_bounded_attempt_with_the_same_payload() {
    let mut store = Store::open_memory().unwrap();
    store.create(&run()).unwrap();
    let first = store.prepare("run-1").unwrap();
    store.freeze(&first.id, &assignment()).unwrap();
    store.begin(&first.id, now_ms()).unwrap();
    store
        .mark_attempt(&first.id, AttemptState::Failed, true, "provider_failure")
        .unwrap();
    let retried = store
        .retry("run-1", "Provider recovered", now_ms())
        .unwrap();
    assert_ne!(retried.attempt.id, first.id);
    assert_ne!(retried.attempt.session_id, first.session_id);
    assert_eq!(retried.dispatch.unwrap().prompt, "Original prompt");
    assert_eq!(
        store.execution(&first.id).unwrap().state,
        AttemptState::Failed
    );
}

#[test]
fn retry_does_not_create_a_new_worker_while_termination_is_unknown() {
    let mut store = Store::open_memory().unwrap();
    store.create(&run()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    store.freeze(&attempt.id, &assignment()).unwrap();
    store.begin(&attempt.id, now_ms()).unwrap();
    store
        .mark_attempt(
            &attempt.id,
            AttemptState::Cancelling,
            false,
            "stop_not_confirmed",
        )
        .unwrap();
    assert!(store.retry("run-1", "Try again", now_ms()).is_err());
    assert_eq!(store.attempts("run-1").unwrap().len(), 1);
}

#[test]
fn launch_budget_is_transactional_and_polling_does_not_consume_it() {
    let mut store = Store::open_memory().unwrap();
    let limits = Limits {
        max_agent_launches: 1,
        ..Limits::default()
    };
    store
        .create_configured(&run(), &json!({"config":{"limits":limits}}))
        .unwrap();
    let attempt = store.prepare("run-1").unwrap();
    store.freeze(&attempt.id, &assignment()).unwrap();
    store.begin(&attempt.id, now_ms()).unwrap();
    store.begin(&attempt.id, now_ms()).unwrap();
    assert_eq!(store.control("run-1").unwrap().launches, 1);
    store
        .mark_attempt(&attempt.id, AttemptState::Failed, true, "provider_failure")
        .unwrap();
    let next = store.retry("run-1", "Try once more", now_ms()).unwrap();
    assert!(store.begin(&next.attempt.id, now_ms()).is_err());
    assert_eq!(store.control("run-1").unwrap().launches, 1);
}

#[test]
fn blocked_attempt_cannot_submit_a_report_or_advance_a_gate() {
    let mut store = Store::open_memory().unwrap();
    store.create(&run()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    store
        .mark_attempt(&attempt.id, AttemptState::Blocked, false, "dirty_workspace")
        .unwrap();
    assert!(
        store
            .complete(
                &attempt.id,
                None,
                Report::Plan {
                    acceptance: vec!["No hang".into()],
                    instructions: "Fix shutdown".into(),
                }
            )
            .is_err()
    );
    assert_eq!(store.load("run-1").unwrap().revision, 0);
}

#[test]
fn undispatched_block_can_be_retried_without_inventing_a_legacy_payload() {
    let mut store = Store::open_memory().unwrap();
    store.create(&run()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    store
        .mark_attempt(
            &attempt.id,
            AttemptState::Blocked,
            true,
            "workspace_blocked",
        )
        .unwrap();
    let retried = store
        .retry("run-1", "Workspace repaired", now_ms())
        .unwrap();
    assert_eq!(retried.attempt.id, attempt.id);
    assert!(retried.dispatch.is_none());
    assert_eq!(retried.state, AttemptState::Prepared);
}

#[test]
fn escalation_resolution_requires_confirmed_stop_and_preserves_quality_gates() {
    let mut store = Store::open_memory().unwrap();
    store.create(&run()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    store.freeze(&attempt.id, &assignment()).unwrap();
    store.begin(&attempt.id, now_ms()).unwrap();
    store
        .halt("run-1", "worker_deadline_exceeded", false)
        .unwrap();
    assert_eq!(
        store.execution(&attempt.id).unwrap().state,
        AttemptState::Cancelling
    );
    assert!(
        store
            .resolve("run-1", "Continue", 0, 0, 0, now_ms())
            .is_err()
    );
    store
        .mark_attempt(
            &attempt.id,
            AttemptState::TimedOut,
            true,
            "worker_deadline_exceeded",
        )
        .unwrap();
    let resolved = store
        .resolve("run-1", "Continue with remaining budget", 0, 0, 0, now_ms())
        .unwrap();
    assert_eq!(resolved.phase, Phase::Planning);
    assert_eq!(resolved.revision, 1);
    assert!(!resolved.engineering_accepted && !resolved.qa_validated);
    assert!(resolved.acceptance.is_empty());
    assert_eq!(resolved.mandatory_checks, vec!["test"]);
    assert_eq!(
        store.execution(&attempt.id).unwrap().state,
        AttemptState::TimedOut
    );
}

#[test]
fn exhausted_budget_requires_an_explicit_audited_extension() {
    let mut store = Store::open_memory().unwrap();
    store.create(&run()).unwrap();
    let future = store.control("run-1").unwrap().deadline_ms + 1;
    store.halt("run-1", "run_deadline_exceeded", false).unwrap();
    assert!(store.resolve("run-1", "Continue", 0, 0, 0, future).is_err());
    let resolved = store
        .resolve("run-1", "Approve one more minute", 60, 0, 0, future)
        .unwrap();
    assert_eq!(resolved.phase, Phase::Planning);
    assert_eq!(store.control("run-1").unwrap().deadline_ms, future + 60_000);
    let event = store.events("run-1").unwrap().pop().unwrap();
    assert_eq!(event.kind, "escalation_resolved");
    assert_eq!(event.payload["additional_seconds"], 60);
}

#[test]
fn cancellation_is_terminal_and_does_not_accept_late_reports() {
    let mut store = Store::open_memory().unwrap();
    store.create(&run()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    store.halt("run-1", "operator_cancelled", true).unwrap();
    assert_eq!(store.load("run-1").unwrap().phase, Phase::Cancelled);
    assert!(store.retry("run-1", "Try again", now_ms()).is_err());
    assert!(
        store
            .resolve("run-1", "Try again", 60, 1, 1, now_ms())
            .is_err()
    );
    assert!(
        store
            .complete(
                &attempt.id,
                None,
                Report::Plan {
                    acceptance: vec!["Pass".into()],
                    instructions: "Implement".into()
                }
            )
            .is_err()
    );
}

struct WrongServer {
    called: std::sync::atomic::AtomicBool,
}

impl synarch::opencode::AgentBackend for WrongServer {
    fn endpoint(&self) -> Option<&str> {
        Some("http://server-b/")
    }
    fn execute(
        &self,
        _: &synarch::store::Attempt,
        _: &Assignment,
    ) -> anyhow::Result<Option<Report>> {
        anyhow::bail!("must not dispatch")
    }
    fn interrupt(
        &self,
        _: &synarch::store::Attempt,
    ) -> anyhow::Result<synarch::opencode::WorkerState> {
        self.called.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(synarch::opencode::WorkerState::Missing)
    }
}

#[test]
fn wrong_server_cannot_falsely_confirm_that_the_original_worker_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    store.create(&run()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    let mut dispatch = assignment();
    dispatch.endpoint = Some("http://server-a/".into());
    store.freeze(&attempt.id, &dispatch).unwrap();
    store.begin(&attempt.id, now_ms()).unwrap();
    drop(store);
    let backend = WrongServer {
        called: false.into(),
    };
    assert!(synarch::recovery::cancel(&db, "run-1", "Stop the worker", Some(&backend)).is_err());
    assert!(!backend.called.load(std::sync::atomic::Ordering::SeqCst));
    let store = Store::open(&db).unwrap();
    assert!(!store.execution(&attempt.id).unwrap().stop_confirmed);
    assert_eq!(
        store.execution(&attempt.id).unwrap().state,
        AttemptState::Cancelling
    );
}

#[test]
fn deadline_is_checked_again_inside_the_report_commit_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.sqlite");
    let mut store = Store::open(&db).unwrap();
    store.create(&run()).unwrap();
    let attempt = store.prepare("run-1").unwrap();
    store.freeze(&attempt.id, &assignment()).unwrap();
    store.begin(&attempt.id, now_ms()).unwrap();
    // Simulate a persisted deadline having elapsed while the controller was waiting
    // to enter its final SQLite transaction, without a real-time sleep in the test.
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute(
            "UPDATE attempts SET execution=json_set(execution,'$.deadline_ms',?2) WHERE id=?1",
            rusqlite::params![attempt.id, (now_ms() - 1) as i64],
        )
        .unwrap();
    assert!(
        store
            .complete(
                &attempt.id,
                None,
                Report::Plan {
                    acceptance: vec!["Pass".into()],
                    instructions: "Implement".into(),
                }
            )
            .is_err()
    );
    assert_eq!(store.load("run-1").unwrap().revision, 0);
}
