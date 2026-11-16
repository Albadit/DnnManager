//! Where a site's keep warm stands, and what a request found - `KeepWarmModel.cs`.

use std::time::Duration;

use crate::rules;

/// Where a site's keep warm stands - what its flame shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeepWarmState {
    /// The site isn't kept warm.
    Off,
    /// Switched on, the first request is coming - DNN Manager has just started, or another site is warming up.
    Waiting,
    /// A warm-up request is on its way: DNN is starting up.
    WarmingUp,
    /// The site answered the last request (or is in use anyway).
    Warm,
    /// Nothing is sent right now - the site or IIS is stopped, an operation runs, a debugger is attached…
    Paused,
    /// The last request failed; it is tried again in a while.
    Failing,
    /// Failed several times in a row - not tried again until the site is started, Check now is pressed or the PC wakes up.
    Stopped,
}

/// A site's keep warm for people: its state and one line about it, e.g. "Warm - answered in 12 ms at 14:02".
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeepWarmStatus {
    pub state: KeepWarmState,
    pub text: String,
}

impl KeepWarmStatus {
    pub fn new(state: KeepWarmState, text: impl Into<String>) -> Self {
        Self { state, text: text.into() }
    }

    pub fn off() -> Self {
        Self::new(KeepWarmState::Off, "Off")
    }

    /// What a site just switched on shows until the service says more.
    pub fn waiting() -> Self {
        Self::new(KeepWarmState::Waiting, "Waiting")
    }

    /// The site is switched to "keep warm", whatever it is doing right now.
    pub fn is_on(&self) -> bool {
        self.state != KeepWarmState::Off
    }
}

/// Something about a site's keep warm worth a line in the activity log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepWarmNotice {
    pub site: String,
    pub message: String,
    pub is_warning: bool,
}

/// What a keep-warm request is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepWarmRequestKind {
    /// The light keep-alive page, to a site that has a worker process.
    Ping,
    /// The warm-up page, to a site without a worker process (or one that answered slowly): DNN starts up.
    WarmUp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeepWarmOutcomeKind {
    /// The site answered - its worker process and DNN are up.
    Ok,
    /// The keep-alive page doesn't exist (404) - the warm-up page is used instead.
    PingPathMissing,
    /// The site answered with an error (HTTP 500, DNN's error page).
    Failed,
    /// No answer: refused, timed out, or HTTP 502/503/504.
    NotAnswering,
    /// Not sent: the site served other requests since the last look - it is in use, so warm anyway.
    InUse,
    /// Not sent: the SQL Server the site's database is on doesn't answer - warming up would fail.
    SqlDown,
}

/// What a request (and the redirects it followed) found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepWarmOutcome {
    pub kind: KeepWarmOutcomeKind,
    /// Until the first response's headers - how long the site took to answer.
    pub elapsed: Duration,
    pub status_code: Option<u16>,
    /// For a failure: what went wrong, for people.
    pub reason: Option<String>,
    /// Something about an answer that is still fine, e.g. DNN's installer waiting.
    pub note: Option<String>,
    /// How many requests were sent (redirects followed count).
    pub requests: u32,
    /// For `InUse` and pings: the site's request counter when it was looked at.
    pub requests_served: Option<u64>,
    /// DNN Manager was in efficiency mode meanwhile: `elapsed` can be longer than the site took.
    pub throttled: bool,
    /// No answer came within the request's time limit.
    pub timed_out: bool,
}

impl KeepWarmOutcome {
    pub fn of(kind: KeepWarmOutcomeKind) -> Self {
        Self {
            kind,
            elapsed: Duration::ZERO,
            status_code: None,
            reason: None,
            note: None,
            requests: 0,
            requests_served: None,
            throttled: false,
            timed_out: false,
        }
    }

    pub fn failed(kind: KeepWarmOutcomeKind, reason: impl Into<String>) -> Self {
        Self { reason: Some(reason.into()), ..Self::of(kind) }
    }

    /// The answer took long enough to say DNN had to start for it.
    pub fn slow(&self) -> bool {
        self.elapsed > if self.throttled { rules::SLOW_AFTER_WHILE_THROTTLED } else { rules::SLOW_AFTER }
    }
}

/// What keep warm holds back while one of DNN Manager's operations runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeepWarmPause {
    /// Nothing is sent at all - IIS itself is being started, stopped or restarted.
    pub everything: bool,
    /// The sites the operation is about: nothing is sent to them; the others are kept warm as usual.
    pub sites: Option<Vec<String>>,
}

impl KeepWarmPause {
    pub fn all() -> Self {
        Self { everything: true, sites: None }
    }

    pub fn for_sites(sites: Vec<String>) -> Self {
        Self { everything: false, sites: Some(sites) }
    }

    /// Another operation (a new project, an import, a backup…): running sites are kept warm, but no site is warmed up
    /// meanwhile - a cold start costs seconds of CPU the operation is using.
    pub fn warm_ups() -> Self {
        Self { everything: false, sites: None }
    }
}
