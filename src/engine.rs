//! Pure transition guards. No model output can directly set workflow state.
use crate::contracts::*;
use anyhow::{Result, bail, ensure};
use std::collections::BTreeSet;

pub fn apply(run: &Run, role: Role, snapshot: Option<&str>, report: Report) -> Result<Run> {
    run.validate()?;
    ensure!(!run.paused, "run is paused");
    ensure!(
        run.phase.role() == Some(role),
        "role is not authorized for this phase"
    );
    if !matches!(run.phase, Phase::Planning | Phase::Implementing) {
        ensure!(
            snapshot.is_some() && snapshot == run.snapshot.as_deref(),
            "stale implementation snapshot"
        );
    }
    let mut next = run.clone();
    match (run.phase, report) {
        (
            Phase::Planning,
            Report::Plan {
                acceptance,
                instructions,
            },
        ) => {
            ensure!(
                !acceptance.is_empty() && acceptance.len() <= 100,
                "acceptance criteria required (maximum 100)"
            );
            unique(acceptance.iter().map(String::as_str))?;
            text(&instructions)?;
            next.acceptance = acceptance;
            next.instructions = instructions;
            next.phase = Phase::Implementing;
        }
        (Phase::Implementing, Report::Implementation { summary }) => {
            text(&summary)?;
            text(snapshot.ok_or_else(|| anyhow::anyhow!("implementation snapshot required"))?)?;
            next.snapshot = snapshot.map(str::to_owned);
            next.checks.clear();
            next.findings.clear();
            next.engineering_accepted = false;
            next.qa_validated = false;
            next.phase = Phase::Checking;
        }
        (Phase::Checking, Report::Checks { results }) => {
            unique(results.iter().map(|r| r.name.as_str()))?;
            let actual: BTreeSet<_> = results.iter().map(|r| &r.name).collect();
            let required: BTreeSet<_> = run.mandatory_checks.iter().collect();
            ensure!(
                actual == required,
                "mandatory check results must match the configured checks exactly"
            );
            for result in &results {
                text(&result.evidence)?;
            }
            next.checks = results;
            next.phase = Phase::Reviewing;
        }
        (Phase::Reviewing, Report::Review { findings }) => {
            validate_findings(&findings)?;
            next.findings = findings;
            next.phase = Phase::Deciding;
        }
        (
            Phase::Deciding | Phase::Triage,
            Report::Decision {
                verdict,
                rationale,
                instructions,
                dispositions,
            },
        ) => {
            text(&rationale)?;
            unique(dispositions.iter().map(|d| d.finding_id.as_str()))?;
            let expected: BTreeSet<_> = run.findings.iter().map(|f| &f.id).collect();
            let actual: BTreeSet<_> = dispositions.iter().map(|d| &d.finding_id).collect();
            ensure!(
                actual == expected,
                "every finding requires exactly one disposition"
            );
            for disposition in &dispositions {
                text(&disposition.rationale)?;
            }
            match verdict {
                Verdict::Accept => {
                    ensure!(
                        run.phase == Phase::Deciding,
                        "QA failure requires triage, not acceptance"
                    );
                    ensure!(
                        run.checks.len() == run.mandatory_checks.len()
                            && run.checks.iter().all(|r| r.passed),
                        "mandatory checks have not passed"
                    );
                    ensure!(
                        dispositions
                            .iter()
                            .all(|d| d.action == FindingAction::Reject),
                        "accepted findings require rework"
                    );
                    next.engineering_accepted = true;
                    next.phase = Phase::Qa;
                }
                Verdict::Rework => {
                    text(&instructions)?;
                    next.instructions = instructions;
                    next.snapshot = None;
                    next.checks.clear();
                    next.findings.clear();
                    next.engineering_accepted = false;
                    next.qa_validated = false;
                    next.phase = Phase::Implementing;
                    consume_iteration(&mut next);
                }
                Verdict::Experiment => {
                    text(&instructions)?;
                    next.instructions = instructions;
                    next.experiment_return = Some(run.phase);
                    next.phase = Phase::Experiment;
                    consume_iteration(&mut next);
                }
                Verdict::RetryQa => {
                    ensure!(
                        run.phase == Phase::Triage,
                        "retry_qa is only valid during QA triage"
                    );
                    ensure!(
                        dispositions
                            .iter()
                            .all(|d| d.action == FindingAction::Reject),
                        "unresolved QA findings require rework"
                    );
                    next.findings.clear();
                    next.phase = Phase::Qa;
                    consume_iteration(&mut next);
                }
                Verdict::Escalate => next.phase = Phase::Escalated,
            }
        }
        (Phase::Experiment, Report::Experiment { evidence }) => {
            text(&evidence)?;
            next.phase = next
                .experiment_return
                .take()
                .ok_or_else(|| anyhow::anyhow!("missing experiment return phase"))?;
        }
        (
            Phase::Qa,
            Report::Qa {
                passed,
                evidence,
                findings,
            },
        ) => {
            ensure!(
                run.engineering_accepted,
                "engineering acceptance required before QA"
            );
            text(&evidence)?;
            validate_findings(&findings)?;
            ensure!(
                !passed || findings.is_empty(),
                "passing QA cannot contain unresolved findings"
            );
            ensure!(
                passed || !findings.is_empty(),
                "failing QA requires reproducible findings"
            );
            next.qa_validated = passed;
            next.findings = findings;
            next.phase = if passed { Phase::Ready } else { Phase::Triage };
        }
        _ => bail!("report does not match the current phase"),
    }
    next.revision += 1;
    Ok(next)
}

fn consume_iteration(run: &mut Run) {
    if run.iteration >= run.max_iterations {
        run.phase = Phase::Escalated;
    } else {
        run.iteration += 1;
    }
}
