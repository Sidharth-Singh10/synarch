use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Jury,
    Actor,
    Critic,
    Qa,
    Controller,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Planning,
    Implementing,
    Checking,
    Reviewing,
    Deciding,
    Qa,
    Triage,
    Experiment,
    Ready,
    Escalated,
    Cancelled,
}

impl Phase {
    pub fn role(self) -> Option<Role> {
        match self {
            Self::Planning | Self::Deciding | Self::Triage => Some(Role::Jury),
            Self::Implementing | Self::Experiment => Some(Role::Actor),
            Self::Checking => Some(Role::Controller),
            Self::Reviewing => Some(Role::Critic),
            Self::Qa => Some(Role::Qa),
            Self::Ready | Self::Escalated | Self::Cancelled => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub id: String,
    pub description: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingAction {
    Fix,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Disposition {
    pub finding_id: String,
    pub action: FindingAction,
    pub rationale: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Accept,
    Rework,
    Experiment,
    RetryQa,
    Escalate,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckResult {
    pub name: String,
    pub passed: bool,
    pub evidence: String,
}

/// The role and snapshot are supplied by the controller, never trusted from model text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Report {
    Plan {
        acceptance: Vec<String>,
        instructions: String,
    },
    Implementation {
        summary: String,
    },
    Checks {
        results: Vec<CheckResult>,
    },
    Review {
        findings: Vec<Finding>,
    },
    Decision {
        verdict: Verdict,
        rationale: String,
        instructions: String,
        dispositions: Vec<Disposition>,
    },
    Qa {
        passed: bool,
        evidence: String,
        findings: Vec<Finding>,
    },
    Experiment {
        evidence: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub id: String,
    pub request: String,
    pub phase: Phase,
    pub revision: u64,
    pub iteration: u32,
    pub max_iterations: u32,
    pub paused: bool,
    pub acceptance: Vec<String>,
    pub instructions: String,
    pub mandatory_checks: Vec<String>,
    pub snapshot: Option<String>,
    pub checks: Vec<CheckResult>,
    pub findings: Vec<Finding>,
    pub engineering_accepted: bool,
    pub qa_validated: bool,
    pub experiment_return: Option<Phase>,
    #[serde(default)]
    pub operator_context: String,
}

impl Run {
    pub fn new(
        id: String,
        request: String,
        mandatory_checks: Vec<String>,
        max_iterations: u32,
    ) -> Self {
        Self {
            id,
            request,
            mandatory_checks,
            max_iterations,
            phase: Phase::Planning,
            revision: 0,
            iteration: 1,
            paused: false,
            acceptance: vec![],
            instructions: String::new(),
            snapshot: None,
            checks: vec![],
            findings: vec![],
            engineering_accepted: false,
            qa_validated: false,
            experiment_return: None,
            operator_context: String::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        text(&self.id)?;
        text(&self.request)?;
        ensure!(
            (1..=100).contains(&self.max_iterations),
            "max_iterations must be 1..=100"
        );
        ensure!(
            !self.mandatory_checks.is_empty(),
            "at least one mandatory check is required"
        );
        unique(self.mandatory_checks.iter().map(String::as_str))
    }
}

pub(crate) fn text(value: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.len() <= 65_536,
        "text must contain 1..=65536 bytes"
    );
    Ok(())
}

pub(crate) fn unique<'a>(values: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        text(value)?;
        ensure!(seen.insert(value), "duplicate identifier: {value}");
    }
    Ok(())
}

pub(crate) fn validate_findings(findings: &[Finding]) -> Result<()> {
    ensure!(findings.len() <= 100, "too many findings");
    unique(findings.iter().map(|f| f.id.as_str()))?;
    for finding in findings {
        text(&finding.description)?;
        text(&finding.evidence)?;
    }
    Ok(())
}
