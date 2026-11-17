//! DNN Manager's records and rules. No dependencies, like `src/DnnManager.Domain`.

mod models;
mod project_name;

pub use models::{DatabaseConfig, DnnProject, DnnRelease, DnnReleaseList};
pub use project_name::{PROJECT_NAME_MAX_LENGTH, validate_project_name};
