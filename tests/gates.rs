use synarch::contracts::*;
use synarch::engine::apply;

fn run() -> Run {
    Run::new(
        "run-1".into(),
        "Fix shutdown".into(),
        vec!["test".into()],
        3,
    )
}

fn plan() -> Report {
    Report::Plan {
        acceptance: vec!["Shutdown flushes pending writes".into()],
        instructions: "Implement and test shutdown".into(),
    }
}

fn advance(run: &Run, role: Role, report: Report) -> Run {
    apply(run, role, run.snapshot.as_deref(), report).unwrap()
}

fn reviewed(passed: bool) -> Run {
    let run = advance(&run(), Role::Jury, plan());
    let run = apply(
        &run,
        Role::Actor,
        Some("commit-a"),
        Report::Implementation {
            summary: "Implemented shutdown flush".into(),
        },
    )
    .unwrap();
    let run = advance(
        &run,
        Role::Controller,
        Report::Checks {
            results: vec![CheckResult {
                name: "test".into(),
                passed,
                evidence: "test output".into(),
            }],
        },
    );
    advance(&run, Role::Critic, Report::Review { findings: vec![] })
}

fn accept() -> Report {
    Report::Decision {
        verdict: Verdict::Accept,
        rationale: "Criteria satisfied".into(),
        instructions: String::new(),
        dispositions: vec![],
    }
}

#[test]
fn happy_path_requires_independent_qa() {
    let run = advance(&reviewed(true), Role::Jury, accept());
    assert_eq!(run.phase, Phase::Qa);
    let run = advance(
        &run,
        Role::Qa,
        Report::Qa {
            passed: true,
            evidence: "Verified shutdown and regression tests".into(),
            findings: vec![],
        },
    );
    assert_eq!(run.phase, Phase::Ready);
}

#[test]
fn actor_cannot_approve_itself() {
    assert!(apply(&reviewed(true), Role::Actor, Some("commit-a"), accept()).is_err());
}

#[test]
fn jury_cannot_override_a_failed_mandatory_check() {
    assert!(apply(&reviewed(false), Role::Jury, Some("commit-a"), accept()).is_err());
}

#[test]
fn missing_or_duplicate_check_results_are_rejected() {
    let run = advance(&run(), Role::Jury, plan());
    let run = apply(
        &run,
        Role::Actor,
        Some("commit-a"),
        Report::Implementation {
            summary: "Implemented".into(),
        },
    )
    .unwrap();
    assert!(
        apply(
            &run,
            Role::Controller,
            Some("commit-a"),
            Report::Checks { results: vec![] }
        )
        .is_err()
    );
    let result = CheckResult {
        name: "test".into(),
        passed: true,
        evidence: "ok".into(),
    };
    assert!(
        apply(
            &run,
            Role::Controller,
            Some("commit-a"),
            Report::Checks {
                results: vec![result.clone(), result],
            }
        )
        .is_err()
    );
}

#[test]
fn stale_snapshot_cannot_receive_acceptance() {
    assert!(apply(&reviewed(true), Role::Jury, Some("commit-b"), accept()).is_err());
}

#[test]
fn qa_failure_requires_jury_triage_and_cannot_be_accepted() {
    let run = advance(&reviewed(true), Role::Jury, accept());
    let run = advance(
        &run,
        Role::Qa,
        Report::Qa {
            passed: false,
            evidence: "Shutdown hangs with two clients".into(),
            findings: vec![Finding {
                id: "Q-1".into(),
                description: "Hang".into(),
                evidence: "Two-client reproduction".into(),
            }],
        },
    );
    assert_eq!(run.phase, Phase::Triage);
    assert!(apply(&run, Role::Jury, Some("commit-a"), accept()).is_err());
    let run = advance(
        &run,
        Role::Jury,
        Report::Decision {
            verdict: Verdict::Rework,
            rationale: "Confirmed hang".into(),
            instructions: "Fix hang".into(),
            dispositions: vec![Disposition {
                finding_id: "Q-1".into(),
                action: FindingAction::Fix,
                rationale: "Reproduced".into(),
            }],
        },
    );
    assert_eq!(run.phase, Phase::Implementing);
    assert!(run.snapshot.is_none());
    assert!(run.checks.is_empty());
    assert!(!run.engineering_accepted);
}

#[test]
fn all_findings_need_explicit_jury_disposition() {
    let mut run = reviewed(true);
    run.findings.push(Finding {
        id: "F-1".into(),
        description: "Race".into(),
        evidence: "src/shutdown.rs:42".into(),
    });
    assert!(apply(&run, Role::Jury, Some("commit-a"), accept()).is_err());
}

#[test]
fn bounded_rework_escalates_instead_of_looping() {
    let mut run = reviewed(true);
    run.iteration = run.max_iterations;
    let run = advance(
        &run,
        Role::Jury,
        Report::Decision {
            verdict: Verdict::Rework,
            rationale: "Still incorrect".into(),
            instructions: "Fix race".into(),
            dispositions: vec![],
        },
    );
    assert_eq!(run.phase, Phase::Escalated);
}

#[test]
fn malformed_reports_and_unknown_fields_fail_closed() {
    assert!(
        serde_json::from_str::<Report>(
            r#"{"kind":"qa","passed":true,"evidence":"ok","findings":[],"override":true}"#
        )
        .is_err()
    );
    assert!(
        apply(
            &run(),
            Role::Jury,
            None,
            Report::Plan {
                acceptance: vec![],
                instructions: "Do something".into(),
            }
        )
        .is_err()
    );
}
