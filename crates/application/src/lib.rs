//! Use cases and the interfaces they see IIS, files and settings through. Depends on the domain only, never on
//! infrastructure or the UI - the layering rule of the C# app (ADR 0002).

pub mod iis;
pub mod operation;
pub mod project;
pub mod records;
pub mod settings;
pub mod source_control;
pub mod use_cases;

pub use dnnm_domain as domain;
