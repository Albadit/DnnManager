//! Keep warm: keeps the IIS sites switched to "keep warm" warm while DNN Manager runs - ports
//! `src/DnnManager.Infrastructure/KeepWarm/**` (the service, its rules, plan and requests), the `keep_warm` records
//! (`Projects/KeepWarmRecords.cs`), the keep-warm part of `KeepWarmSettings` (`UserSettings.cs`) and how the WPF app
//! shows it (`ProjectRow.cs`, the `KeepWarmFlame` style, `FlameOutline`/`FlameFilled` and the overview's Keep warm card).
//!
//! The app starts one [`KeepWarmService`], feeds it IIS's state and the sites as the monitor reads them, and shows what
//! [`KeepWarmService::status_of`] (or the [`KeepWarmEvent`]s) say - with the texts of [`view`].

mod debugger;
mod local_time;
pub mod model;
pub mod plan;
pub mod records;
pub mod requester;
pub mod rules;
pub mod service;
#[cfg(test)]
mod service_tests;
pub mod settings;
pub mod sql_address;
pub mod view;

pub use model::{
    KeepWarmNotice, KeepWarmOutcome, KeepWarmOutcomeKind, KeepWarmPause, KeepWarmRequestKind, KeepWarmState,
    KeepWarmStatus,
};
pub use plan::{KeepWarmPlan, LocalSiteTarget};
pub use records::KeepWarmRecords;
pub use service::{KeepWarmConfig, KeepWarmEvent, KeepWarmService, KeepWarmSite};
pub use settings::KeepWarmSettings;
