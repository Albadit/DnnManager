//! A site's Details view (`ProjectView`'s tabs General, IIS, DNN, Database and Advanced) as plain data: what is
//! read about the site (`src/DnnManager.Infrastructure/Diagnostics`: web.config, assemblies, folders, the database)
//! and how `ProjectDiagnostics.cs` turns it into sections and rows - and the issues it finds.
//!
//! How the app uses it:
//!
//! 1. On a background thread, [`snapshot::read_local`] (IIS, web.config, folders, bin, git, backups) - show it.
//! 2. When [`ProjectSnapshot::needs_database`], read the database with its own [`database::DatabaseInspector`]
//!    (the SQL client is the app's), then [`ProjectSnapshot::with_database`] - show it again.
//! 3. [`tabs::tabs`] (or [`tabs::issues`] once and [`tabs::tab`] for the tab shown) gives the sections to draw.
//!    Until the first snapshot, every tab shows [`model::reading`].
//!
//! How to draw a [`model::Section`] (`InspectorPanel`): a card; its title, and `source` muted and small on the right;
//! then its rows - the label muted in a 210 px column, a 7 px dot for the row's [`model::Health`] (none for
//! `Health::None`), the value selectable in the health's colour (Bad: error colour, semibold; Warning: the warning
//! colour), a value that is an address ([`model::Row::link`]) a link, and `detail` muted on a line under it; then
//! the table - muted small headers, a line, a dot column when any row has a health, the last column taking the rest,
//! an address cell a link, a row's `detail` the tooltip of its last cell; then the note, muted, wrapped.

pub mod assemblies;
pub mod database;
pub mod folders;
pub mod format;
pub mod metadata;
pub mod model;
pub mod snapshot;
pub mod tabs;
pub mod web_config;

pub use model::{Health, Row, Section, Tab, TabKind, Table, TableRow};
pub use snapshot::ProjectSnapshot;
pub use tabs::DetectedIssue;

#[cfg(test)]
mod tests;
