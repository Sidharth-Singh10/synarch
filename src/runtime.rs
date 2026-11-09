//! Operational state is separate from technical acceptance and cannot approve code.
use crate::{
    contracts::{Phase, text},
    opencode::Assignment,
    store::Attempt,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock predates Unix epoch")
        .as_millis()
        .try_into()
        .expect("timestamp overflow")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Limits {
    pub worker_timeout_seconds: u64,
    pub run_timeout_seconds: u64,
    pub max_agent_launches: u32,
    pub max_attempts_per_stage: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            worker_timeout_seconds: 900,
            run_timeout_seconds: 3600,
            max_agent_launches: 30,
            max_attempts_per_stage: 3,
        }
    }
}

impl Limits {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=86400).contains(&self.worker_timeout_seconds),
            "worker timeout must be 1..=86400 seconds"
        );
        ensure!(
            (1..=604800).contains(&self.run_timeout_seconds),
            "run timeout must be 1..=604800 seconds"
        );
        ensure!(
            (1..=1000).contains(&self.max_agent_launches),
            "launch budget must be 1..=1000"
        );
        ensure!(
            (1..=10).contains(&self.max_attempts_per_stage),
            "stage attempt limit must be 1..=10"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Prepared,
    Running,
    Succeeded,
    Failed,
    Blocked,
    Cancelling,
    Interrupted,
    TimedOut,
}

impl AttemptState {
    pub fn executable(self) -> bool {
        matches!(self, Self::Prepared | Self::Running)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub attempt: Attempt,
    pub sequence: u32,
    pub state: AttemptState,
    pub dispatch: Option<Assignment>,
    pub started_ms: Option<u64>,
    pub deadline_ms: Option<u64>,
    pub stop_confirmed: bool,
    pub failure_code: Option<String>,
    #[serde(default)]
    pub legacy_unfrozen: bool,
}

impl Execution {
    pub fn new(attempt: Attempt, sequence: u32) -> Self {
        Self {
            attempt,
            sequence,
            state: AttemptState::Prepared,
            dispatch: None,
            started_ms: None,
            deadline_ms: None,
            stop_confirmed: true,
            failure_code: None,
            legacy_unfrozen: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub created_ms: u64,
    pub deadline_ms: u64,
    pub launches: u32,
    pub limits: Limits,
    pub halted_phase: Option<Phase>,
    pub halt_reason: Option<String>,
    pub cancel_requested: bool,
}

impl Control {
    pub fn new(limits: Limits, now: u64) -> Result<Self> {
        limits.validate()?;
        Ok(Self {
            created_ms: now,
            deadline_ms: now
                .checked_add(limits.run_timeout_seconds * 1000)
                .ok_or_else(|| anyhow::anyhow!("deadline overflow"))?,
            launches: 0,
            limits,
            halted_phase: None,
            halt_reason: None,
            cancel_requested: false,
        })
    }
}

pub(crate) fn failure_code(code: &str) -> Result<()> {
    text(code)?;
    ensure!(
        code.len() <= 128
            && code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')),
        "failure code must be bounded inert text"
    );
    Ok(())
}
