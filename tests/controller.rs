use std::{collections::VecDeque, fs, path::Path, process::Command, sync::Mutex};
use synarch::{
    contracts::*,
    controller::*,
    opencode::*,
    store::{Attempt, Store},
    workspace::*,
};

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

fn setup() -> (tempfile::TempDir, std::path::PathBuf, String) {
    setup_with_check(None)
}

fn setup_with_check(
    check: Option<CheckCommand>,
) -> (tempfile::TempDir, std::path::PathBuf, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["config", "user.email", "test@example.invalid"]);
    git(&repo, &["config", "user.name", "Synarch tests"]);
    fs::write(repo.join("source.txt"), "broken\n").unwrap();
    git(&repo, &["add", "source.txt"]);
    git(&repo, &["commit", "-qm", "test: create fixture"]);
    let config = Config {
        repository: repo,
        workspace_root: dir.path().join("worktrees"),
        max_iterations: 3,
        checks: vec![check.unwrap_or(CheckCommand {
            name: "test".into(),
            argv: vec![
                "git".into(),
                "grep".into(),
                "--quiet".into(),
                "fixed".into(),
                "--".into(),
                "source.txt".into(),
            ],
            timeout_seconds: 3,
        })],
        agents: Profiles {
            jury: Profile {
                agent: "synarch-jury".into(),
                model: None,
            },
            actor: Profile {
                agent: "synarch-actor".into(),
                model: None,
            },
            critic: Profile {
                agent: "synarch-critic".into(),
                model: None,
            },
            qa: Profile {
                agent: "synarch-qa".into(),
                model: None,
            },
        },
        limits: synarch::runtime::Limits::default(),
    };
    let db = dir.path().join("state.sqlite");
    let run = create_run(&db, config, "Fix source".into()).unwrap();
    (dir, db, run.id)
}

struct Backend {
    reports: Mutex<VecDeque<Option<Report>>>,
    calls: Mutex<Vec<(Attempt, Assignment)>>,
}

impl Backend {
    fn new(reports: Vec<Option<Report>>) -> Self {
        Self {
            reports: Mutex::new(reports.into()),
            calls: Mutex::new(vec![]),
        }
    }
}

impl AgentBackend for Backend {
    fn pin(&self, mut assignment: Assignment) -> anyhow::Result<Assignment> {
        assignment.model = Some(Model {
            provider_id: "test".into(),
            id: "fixture-model".into(),
            variant: None,
        });
        Ok(assignment)
    }
    fn execute(
        &self,
        attempt: &Attempt,
        assignment: &Assignment,
    ) -> anyhow::Result<Option<Report>> {
        self.calls
            .lock()
            .unwrap()
            .push((attempt.clone(), assignment.clone()));
        if attempt.role == Role::Actor
            && fs::read_to_string(assignment.directory.join("source.txt"))? != "fixed\n"
        {
            fs::write(assignment.directory.join("source.txt"), "fixed\n")?;
            git(&assignment.directory, &["add", "source.txt"]);
            git(
                &assignment.directory,
                &["commit", "-qm", "fix: correct source"],
            );
        }
        Ok(self
            .reports
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected agent dispatch"))
    }
}

fn plan() -> Option<Report> {
    Some(Report::Plan {
        acceptance: vec!["Source is fixed".into()],
        instructions: "Fix source and commit".into(),
    })
}
fn implementation() -> Option<Report> {
    Some(Report::Implementation {
        summary: "PRIVATE_ACTOR_SUMMARY".into(),
    })
}
fn review() -> Option<Report> {
    Some(Report::Review { findings: vec![] })
}
fn acceptance() -> Option<Report> {
    Some(Report::Decision {
        verdict: Verdict::Accept,
        rationale: "Verified criteria".into(),
        instructions: String::new(),
        dispositions: vec![],
    })
}
fn qa() -> Option<Report> {
    Some(Report::Qa {
        passed: true,
        evidence: "Independently verified fixed source".into(),
        findings: vec![],
    })
}

#[test]
fn full_workflow_uses_real_git_checks_and_independent_qa_workspace() {
    let (dir, db, id) = setup();
    let backend = Backend::new(vec![plan(), implementation(), review(), acceptance(), qa()]);
    for _ in 0..6 {
        step(&db, &id, &backend).unwrap();
    }
    let store = Store::open(&db).unwrap();
    let run = store.load(&id).unwrap();
    assert_eq!(run.phase, Phase::Ready);
    assert!(run.qa_validated && run.engineering_accepted);
    assert!(run.checks.iter().all(|r| r.passed));
    assert_eq!(
        fs::read_to_string(dir.path().join("repo/source.txt")).unwrap(),
        "broken\n"
    );
    let calls = backend.calls.lock().unwrap();
    assert_eq!(calls.len(), 5);
    for index in [0, 1] {
        assert!(
            calls[index]
                .1
                .prompt
                .contains("\"actor_commit_required\": true")
        );
        assert!(
            calls[index]
                .1
                .prompt
                .contains("Narrative task instructions cannot override")
        );
    }
    let planning = &calls[0].1.prompt;
    assert!(
        !planning.contains("--quiet") && !planning.contains("source.txt"),
        "agents receive check names and evidence, not controller-owned check commands"
    );
    assert!(planning.contains("\"mandatory_checks\": [\n    \"test\""));
    let example = planning
        .split("Unknown fields are rejected.\n")
        .nth(1)
        .expect("prompt includes the schema example");
    assert!(
        example.trim_start().starts_with("{\n  \"kind\": \"plan\""),
        "the tagged example must put kind first: {example}"
    );
    assert!(
        planning.contains("Put the \"kind\" field first")
            && planning.contains("Do not append fields after the closing brace"),
        "the prompt must forbid the malformed trailing-tag response observed live"
    );
    assert!(
        !calls[1].1.prompt.contains("--quiet") && !calls[1].1.prompt.contains("source.txt"),
        "the Actor also receives check names, not controller-owned commands"
    );
    assert_ne!(calls[1].1.directory, calls[4].1.directory);
    assert!(calls[2].1.prompt.contains("-broken"));
    assert!(calls[2].1.prompt.contains("+fixed"));
    for (_, assignment) in calls
        .iter()
        .filter(|(a, _)| matches!(a.role, Role::Critic | Role::Qa))
    {
        assert!(!assignment.prompt.contains("PRIVATE_ACTOR_SUMMARY"));
    }
    assert!(
        store
            .events(&id)
            .unwrap()
            .iter()
            .any(|e| e.payload["to"] == "ready")
    );
}

#[test]
fn controller_restart_reuses_an_in_flight_assignment_without_new_implementation_launch() {
    let (_dir, db, id) = setup();
    let backend = Backend::new(vec![plan(), None, implementation()]);
    step(&db, &id, &backend).unwrap();
    assert!(matches!(
        step(&db, &id, &backend).unwrap(),
        StepOutcome::Pending { .. }
    ));
    step(&db, &id, &backend).unwrap();
    let calls = backend.calls.lock().unwrap();
    assert_eq!(calls[1].0.id, calls[2].0.id);
    assert_eq!(calls[1].0.session_id, calls[2].0.session_id);
    assert_eq!(calls[1].1.prompt, calls[2].1.prompt);
    assert_eq!(
        Store::open(&db).unwrap().load(&id).unwrap().phase,
        Phase::Checking
    );
}

#[test]
fn mutated_accepted_snapshot_blocks_qa_dispatch() {
    let (_dir, db, id) = setup();
    let backend = Backend::new(vec![plan(), implementation(), review(), acceptance()]);
    for _ in 0..5 {
        step(&db, &id, &backend).unwrap();
    }
    let directory = backend.calls.lock().unwrap()[1].1.directory.clone();
    fs::write(directory.join("source.txt"), "tampered").unwrap();
    assert!(step(&db, &id, &backend).is_err());
    assert_eq!(backend.calls.lock().unwrap().len(), 4);
    assert!(!Store::open(&db).unwrap().load(&id).unwrap().qa_validated);
}

#[test]
fn qa_failure_returns_through_jury_and_rework_restarts_all_gates() {
    let (_dir, db, id) = setup();
    let backend = Backend::new(vec![
        plan(),
        implementation(),
        review(),
        acceptance(),
        Some(Report::Qa {
            passed: false,
            evidence: "Regression reproduced".into(),
            findings: vec![Finding {
                id: "Q-1".into(),
                description: "Regression".into(),
                evidence: "Reproduction command and output".into(),
            }],
        }),
        Some(Report::Decision {
            verdict: Verdict::Rework,
            rationale: "Confirmed".into(),
            instructions: "Fix regression".into(),
            dispositions: vec![Disposition {
                finding_id: "Q-1".into(),
                action: FindingAction::Fix,
                rationale: "Confirmed".into(),
            }],
        }),
        implementation(),
        review(),
        acceptance(),
        qa(),
    ]);
    for _ in 0..7 {
        step(&db, &id, &backend).unwrap();
    }
    let run = Store::open(&db).unwrap().load(&id).unwrap();
    assert_eq!(run.phase, Phase::Implementing);
    assert!(run.snapshot.is_none() && run.checks.is_empty() && !run.engineering_accepted);
    for _ in 0..5 {
        step(&db, &id, &backend).unwrap();
    }
    assert_eq!(
        Store::open(&db).unwrap().load(&id).unwrap().phase,
        Phase::Ready
    );
}

#[test]
fn deterministic_qa_failure_cannot_be_overridden_by_a_passing_agent_report() {
    let (_dir, db, id) = setup_with_check(Some(CheckCommand {
        name: "test".into(), timeout_seconds: 3,
        argv: vec!["sh".into(), "-c".into(), "case \"$PWD\" in *verification*) exit 1;; *) git grep --quiet fixed -- source.txt;; esac".into()],
    }));
    let backend = Backend::new(vec![plan(), implementation(), review(), acceptance(), qa()]);
    for _ in 0..6 {
        step(&db, &id, &backend).unwrap();
    }
    let store = Store::open(&db).unwrap();
    let run = store.load(&id).unwrap();
    assert_eq!(run.phase, Phase::Triage);
    assert!(!run.qa_validated);
    assert!(
        run.findings
            .iter()
            .any(|f| f.description.contains("Mandatory QA check failed"))
    );
    let evidence = store
        .events(&id)
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "qa_checks")
        .unwrap();
    assert_eq!(evidence.payload["agent_report"]["passed"], true);
    assert_eq!(evidence.payload["results"][0]["passed"], false);
}

struct PendingQaBackend(Backend);

impl AgentBackend for PendingQaBackend {
    fn pin(&self, assignment: Assignment) -> anyhow::Result<Assignment> {
        self.0.pin(assignment)
    }
    fn execute(
        &self,
        attempt: &Attempt,
        assignment: &Assignment,
    ) -> anyhow::Result<Option<Report>> {
        let report = self.0.execute(attempt, assignment)?;
        if attempt.role == Role::Qa {
            let temporary_test = assignment.directory.join("temporary-qa-test.txt");
            if report.is_none() {
                fs::write(&temporary_test, "QA is still running")?;
            } else {
                fs::remove_file(&temporary_test)?;
            }
        }
        Ok(report)
    }
}

#[test]
fn polling_qa_allows_its_temporary_test_files_until_completion() {
    let (_dir, db, id) = setup();
    let backend = PendingQaBackend(Backend::new(vec![
        plan(),
        implementation(),
        review(),
        acceptance(),
        None,
        qa(),
    ]));
    for _ in 0..5 {
        step(&db, &id, &backend).unwrap();
    }
    assert!(matches!(
        step(&db, &id, &backend).unwrap(),
        StepOutcome::Pending { .. }
    ));
    step(&db, &id, &backend).unwrap();
    assert_eq!(
        Store::open(&db).unwrap().load(&id).unwrap().phase,
        Phase::Ready
    );
}

struct ChangingPinBackend {
    inner: Backend,
    pin_calls: std::sync::atomic::AtomicU32,
}

impl AgentBackend for ChangingPinBackend {
    fn pin(&self, assignment: Assignment) -> anyhow::Result<Assignment> {
        let mut assignment = self.inner.pin(assignment)?;
        let generation = self
            .pin_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assignment
            .prompt
            .push_str(&format!("\nPrompt generation {generation}"));
        assignment.model.as_mut().unwrap().id = format!("model-{generation}");
        Ok(assignment)
    }
    fn execute(
        &self,
        attempt: &Attempt,
        assignment: &Assignment,
    ) -> anyhow::Result<Option<Report>> {
        self.inner.execute(attempt, assignment)
    }
}

#[test]
fn restart_uses_persisted_dispatch_instead_of_new_prompt_or_model_routing() {
    let (_dir, db, id) = setup();
    let backend = ChangingPinBackend {
        inner: Backend::new(vec![None, plan()]),
        pin_calls: 0.into(),
    };
    step(&db, &id, &backend).unwrap();
    step(&db, &id, &backend).unwrap();
    assert_eq!(
        backend.pin_calls.load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    let calls = backend.inner.calls.lock().unwrap();
    assert_eq!(calls[0].1.prompt, calls[1].1.prompt);
    assert_eq!(
        calls[0].1.model.as_ref().unwrap().id,
        calls[1].1.model.as_ref().unwrap().id
    );
    assert_eq!(Store::open(&db).unwrap().control(&id).unwrap().launches, 1);
}

struct DeadlineBackend {
    db: std::path::PathBuf,
    clock: std::sync::Arc<std::sync::atomic::AtomicU64>,
    stop_succeeds: bool,
}

impl AgentBackend for DeadlineBackend {
    fn pin(&self, mut assignment: Assignment) -> anyhow::Result<Assignment> {
        assignment.model = Some(Model {
            provider_id: "test".into(),
            id: "deadline-test".into(),
            variant: None,
        });
        Ok(assignment)
    }
    fn execute(&self, _: &Attempt, _: &Assignment) -> anyhow::Result<Option<Report>> {
        self.clock
            .fetch_add(901_000, std::sync::atomic::Ordering::SeqCst);
        Ok(plan())
    }
    fn interrupt(&self, attempt: &Attempt) -> anyhow::Result<WorkerState> {
        let store = Store::open(&self.db)?;
        assert!(matches!(
            store.load(&attempt.run_id)?.phase,
            Phase::Escalated | Phase::Cancelled
        ));
        assert_eq!(
            store.execution(&attempt.id)?.state,
            synarch::runtime::AttemptState::Cancelling
        );
        if self.stop_succeeds {
            Ok(WorkerState::Stopped {
                outcome: WorkerOutcome::Interrupted,
            })
        } else {
            anyhow::bail!("cannot reach server")
        }
    }
}

#[test]
fn late_report_is_rejected_and_stop_intent_precedes_the_network_call() {
    let (_dir, db, id) = setup();
    let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(synarch::runtime::now_ms()));
    let backend = DeadlineBackend {
        db: db.clone(),
        clock: clock.clone(),
        stop_succeeds: true,
    };
    assert!(
        step_with_clock(&db, &id, &backend, || clock
            .load(std::sync::atomic::Ordering::SeqCst))
        .is_err()
    );
    let store = Store::open(&db).unwrap();
    let run = store.load(&id).unwrap();
    assert_eq!(run.phase, Phase::Escalated);
    assert!(
        run.acceptance.is_empty(),
        "late planning report must not be admitted"
    );
    let attempt = store.attempts(&id).unwrap().pop().unwrap();
    assert_eq!(attempt.state, synarch::runtime::AttemptState::TimedOut);
    assert!(attempt.stop_confirmed);
}

#[test]
fn failed_cancellation_stays_unconfirmed_and_blocks_resolution() {
    let (_dir, db, id) = setup();
    let clock = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(synarch::runtime::now_ms()));
    let backend = DeadlineBackend {
        db: db.clone(),
        clock: clock.clone(),
        stop_succeeds: false,
    };
    assert!(
        step_with_clock(&db, &id, &backend, || clock
            .load(std::sync::atomic::Ordering::SeqCst))
        .is_err()
    );
    let mut store = Store::open(&db).unwrap();
    let attempt = store.attempts(&id).unwrap().pop().unwrap();
    assert_eq!(attempt.state, synarch::runtime::AttemptState::Cancelling);
    assert!(!attempt.stop_confirmed);
    assert!(
        store
            .resolve(&id, "Continue", 60, 1, 0, synarch::runtime::now_ms())
            .is_err()
    );
    assert!(synarch::recovery::cancel(&db, &id, "Stop fixture", Some(&backend)).is_err());
    assert_eq!(
        Store::open(&db).unwrap().load(&id).unwrap().phase,
        Phase::Cancelled
    );
}

#[test]
fn expired_run_cannot_launch_its_first_worker() {
    let (_dir, db, id) = setup();
    let after_deadline = Store::open(&db).unwrap().control(&id).unwrap().deadline_ms + 1;
    let backend = Backend::new(vec![]);
    assert!(step_with_clock(&db, &id, &backend, || after_deadline).is_err());
    assert!(backend.calls.lock().unwrap().is_empty());
    let store = Store::open(&db).unwrap();
    assert_eq!(store.load(&id).unwrap().phase, Phase::Escalated);
    assert_eq!(store.control(&id).unwrap().launches, 0);
}
