//! DNN's upgrade path, what is known about each step, and the pre-upgrade analyser's rules
//! (`src/DnnManager.Application/Upgrades/**`). Pure: the facts are read by the site inspector and the upgrade checks.

pub mod analyser;
pub mod comparison;
pub mod diagnosis;
pub mod facts;
pub mod knowledge;
pub mod path;

pub use analyser::{DnnUpgradePlan, PlannedStep, UpgradeFinding, UpgradeFindingSeverity, plan};
pub use comparison::{ContentOutcome, DnnPageState, DnnSiteState, DnnTestAccount, compare};
pub use diagnosis::{UpgradeDiagnosis, explain};
pub use facts::{
    DnnAssembly, DnnContentCounts, DnnContentItem, DnnExtension, DnnSiteFacts, net_framework_name, sql_server_name,
};
pub use knowledge::STEPS;
pub use path::{
    DnnPathVersion, DnnRequirement, DnnRequirementKind, DnnUpgradeMethod, DnnUpgradeStep, MAX_UPGRADE_STOP_NOTE,
    NET_FRAMEWORK_CHOICES, OLDEST_ON_GITHUB, SOURCE, SQL_SERVER_CHOICES, UpgradeStop, VERSIONS, chain, edits_for,
    is_manual, listed, listed_minimum, method_for, path_with, upgrade_stop_version, upgrade_stops_problem,
};
