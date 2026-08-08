//! Durable state machine for one scheduled occurrence.

use super::*;

mod claim;
mod finish;
mod start;

#[derive(Clone)]
pub struct ThreadScheduleClaim {
    pub schedule: crate::ThreadSchedule,
    pub run: crate::ThreadScheduleRun,
    pub occurrence_state: ThreadScheduleOccurrenceState,
    pub turn_input: Option<String>,
    pub occurrence_auth_profile: Option<Option<String>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadScheduleOccurrenceState {
    WaitingIdle,
    Enqueued,
    Started,
    Terminal,
}

#[derive(Clone)]
pub struct ThreadScheduleDueClaimParams<'a> {
    pub now: DateTime<Utc>,
    pub lease_id: &'a str,
    pub lease_duration: Duration,
    pub local_active_owner_id: Option<&'a str>,
    pub local_active_fresh_after: Option<DateTime<Utc>>,
}

#[derive(Clone)]
pub struct ThreadScheduleNowClaimParams<'a> {
    pub schedule_id: &'a str,
    pub now: DateTime<Utc>,
    pub lease_id: &'a str,
    pub lease_duration: Duration,
    pub local_active_owner_id: Option<&'a str>,
    pub local_active_fresh_after: Option<DateTime<Utc>>,
}

pub struct ThreadScheduleRunForGoalFinishParams<'a> {
    pub schedule_id: &'a str,
    pub run_id: &'a str,
    pub lease_id: &'a str,
    pub completed_at: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub expected_goal_id: &'a str,
}

#[derive(Clone)]
pub struct ThreadScheduleRunStartParams<'a> {
    pub schedule_id: &'a str,
    pub run_id: &'a str,
    pub lease_id: &'a str,
    pub turn_id: &'a str,
    pub goal_id: Option<&'a str>,
    pub now: DateTime<Utc>,
    pub lease_duration: Duration,
}

#[derive(Clone)]
pub struct ThreadScheduleRunEnqueueParams<'a> {
    pub schedule_id: &'a str,
    pub run_id: &'a str,
    pub lease_id: &'a str,
    pub goal_id: Option<&'a str>,
    pub auth_profile_recorded: bool,
    pub auth_profile: Option<&'a str>,
    pub turn_input: &'a str,
    pub now: DateTime<Utc>,
}

#[derive(Clone)]
pub struct ThreadScheduleRunLeaseParams<'a> {
    pub schedule_id: &'a str,
    pub run_id: &'a str,
    pub lease_id: &'a str,
    pub now: DateTime<Utc>,
    pub lease_duration: Duration,
}

#[derive(Clone, sqlx::FromRow)]
struct ThreadScheduleOccurrenceRow {
    occurrence_id: String,
    schedule_id: String,
    thread_id: String,
    state: String,
    turn_id: String,
    goal_id: Option<String>,
    auth_profile_recorded: bool,
    auth_profile: Option<String>,
    scheduled_for_ms: Option<i64>,
    turn_input: Option<String>,
    created_at_ms: i64,
}

const OCCURRENCE_WAITING_IDLE: &str = "waiting_idle";
const OCCURRENCE_ENQUEUED: &str = "enqueued";
const OCCURRENCE_STARTED: &str = "started";
const OCCURRENCE_TERMINAL: &str = "terminal";

impl ThreadScheduleOccurrenceState {
    fn from_str(state: &str) -> anyhow::Result<Self> {
        match state {
            OCCURRENCE_WAITING_IDLE => Ok(Self::WaitingIdle),
            OCCURRENCE_ENQUEUED => Ok(Self::Enqueued),
            OCCURRENCE_STARTED => Ok(Self::Started),
            OCCURRENCE_TERMINAL => Ok(Self::Terminal),
            state => anyhow::bail!("unsupported thread schedule occurrence state {state}"),
        }
    }
}

#[derive(Clone, Copy)]
enum ThreadScheduleClaimTarget<'a> {
    Due,
    Now { schedule_id: &'a str },
}

#[derive(Clone)]
struct ClaimThreadScheduleParams<'a> {
    target: ThreadScheduleClaimTarget<'a>,
    now: DateTime<Utc>,
    lease_id: &'a str,
    lease_duration: Duration,
    local_active_owner_id: Option<&'a str>,
    local_active_fresh_after: Option<DateTime<Utc>>,
}

#[derive(Clone)]
struct FinishThreadScheduleRunParams<'a> {
    schedule_id: &'a str,
    run_id: &'a str,
    lease_id: &'a str,
    completed_at: DateTime<Utc>,
    next_run_at: Option<DateTime<Utc>>,
    expected_goal_id: Option<&'a str>,
    finish: FinishScheduleRun,
}

#[derive(Clone)]
struct FinalizeThreadScheduleRunParams<'a> {
    schedule_id: &'a str,
    run_id: &'a str,
    lease_id: &'a str,
    completed_at: DateTime<Utc>,
    next_run_at: Option<DateTime<Utc>>,
    expected_goal_id: Option<&'a str>,
}

#[derive(Clone)]
enum FinishScheduleRun {
    Completed,
    Failed { error: String },
}
