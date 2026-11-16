//! The Projects table's optional columns (`Pages/Projects/ProjectColumns.cs`): key as saved in
//! `appearance.projectColumns`, the name in the Columns menu, the header, the width and how it sorts.

use dnnm_application::project::{ProjectState, view};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    Url,
    Dnn,
    Database,
    Sql,
    Cpu,
    Memory,
    Pid,
    LastStarted,
    Status,
    Ports,
    Id,
    Size,
    MemoryPercent,
    Disk,
    Network,
    Path,
}

impl Column {
    /// Menu order = table order.
    pub const ALL: [Column; 16] = [
        Column::Url,
        Column::Dnn,
        Column::Database,
        Column::Sql,
        Column::Cpu,
        Column::Memory,
        Column::Pid,
        Column::LastStarted,
        Column::Status,
        Column::Ports,
        Column::Id,
        Column::Size,
        Column::MemoryPercent,
        Column::Disk,
        Column::Network,
        Column::Path,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Column::Url => "url",
            Column::Dnn => "dnn",
            Column::Database => "database",
            Column::Sql => "sql",
            Column::Cpu => "cpu",
            Column::Memory => "memory",
            Column::Pid => "pid",
            Column::LastStarted => "lastStarted",
            Column::Status => "status",
            Column::Ports => "ports",
            Column::Id => "id",
            Column::Size => "size",
            Column::MemoryPercent => "memoryPercent",
            Column::Disk => "disk",
            Column::Network => "network",
            Column::Path => "path",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.key() == key)
    }

    /// The name in the Columns menu.
    pub fn menu_name(self) -> &'static str {
        match self {
            Column::Url => "Site",
            Column::Dnn => "DNN version",
            Column::Database => "Database",
            Column::Sql => "SQL",
            Column::Cpu => "CPU (%)",
            Column::Memory => "Memory usage",
            Column::Pid => "PID",
            Column::LastStarted => "Last started",
            Column::Status => "Status",
            Column::Ports => "Port(s)",
            Column::Id => "Site ID",
            Column::Size => "Size",
            Column::MemoryPercent => "Memory (%)",
            Column::Disk => "Disk read/write",
            Column::Network => "Network I/O",
            Column::Path => "Path",
        }
    }

    pub fn header(self) -> &'static str {
        match self {
            Column::Url => "Site",
            Column::Dnn => "DNN",
            Column::Memory => "Memory",
            other => other.menu_name(),
        }
    }

    /// The XAML column's MinWidth; the column grows to its widest text, as WPF's `Width="Auto"` columns do.
    pub fn min_width(self) -> f32 {
        match self {
            Column::Url => 180.0,
            Column::Dnn => 70.0,
            Column::Database => 120.0,
            Column::Sql => 64.0,
            Column::Cpu => 70.0,
            Column::Memory => 84.0,
            Column::Pid => 60.0,
            Column::LastStarted => 100.0,
            Column::Status => 90.0,
            Column::Ports => 70.0,
            Column::Id => 64.0,
            Column::Size => 80.0,
            Column::MemoryPercent => 84.0,
            Column::Disk => 130.0,
            Column::Network => 130.0,
            Column::Path => 200.0,
        }
    }

    /// Right-aligned figures (`Numeric`).
    pub fn numeric(self) -> bool {
        matches!(
            self,
            Column::Cpu | Column::Memory | Column::Size | Column::MemoryPercent | Column::Disk | Column::Network
        )
    }

    pub fn text(self, p: &ProjectState) -> String {
        match self {
            Column::Url => view::url(p),
            Column::Dnn => view::dnn(p),
            Column::Database => view::database(p),
            Column::Sql => view::sql(p).into(),
            Column::Cpu => view::cpu(p),
            Column::Memory => view::memory(p),
            Column::Pid => view::pids(&p.site),
            Column::LastStarted => view::last_started(p),
            Column::Status => view::state_text(&p.site).into(),
            Column::Ports => view::ports(&p.site),
            Column::Id => p.site.id.to_string(),
            Column::Size => view::size(p),
            Column::MemoryPercent => view::memory_percent(p),
            Column::Disk => view::disk(p),
            Column::Network => view::network(p),
            Column::Path => p.directory.display().to_string(),
        }
    }
}

/// What the table is sorted by: a column, or the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    State,
    Column(Column),
}

impl SortBy {
    /// The WPF table's name for it (the column's `SortMemberPath`), saved as `projects.sortBy`.
    pub fn key(self) -> &'static str {
        match self {
            SortBy::Name => "Name",
            SortBy::State | SortBy::Column(Column::Status) => "State",
            SortBy::Column(c) => match c {
                Column::Url => "Url",
                Column::Dnn => "Dnn",
                Column::Database => "Database",
                Column::Sql => "Sql",
                Column::Cpu => "CpuSort",
                Column::Memory | Column::MemoryPercent => "MemorySort",
                Column::Pid => "PidText",
                Column::LastStarted => "LastStartedSort",
                Column::Ports => "PortsText",
                Column::Id => "IdSort",
                Column::Size => "SizeBytes",
                Column::Disk => "DiskSort",
                Column::Network => "NetworkSort",
                Column::Path => "Path",
                Column::Status => "State",
            },
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        if key == "Name" {
            return Some(SortBy::Name);
        }
        if key == "State" {
            return Some(SortBy::State);
        }
        Column::ALL.into_iter().map(SortBy::Column).find(|s| s.key() == key)
    }
}

/// Compares two projects for a sort; figures not known (`-`) sort below every known one.
pub fn compare(by: SortBy, a: &ProjectState, b: &ProjectState) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
    let figure = |f: &dyn Fn(&ProjectState) -> Option<f64>| {
        let (x, y) = (f(a).unwrap_or(-1.0), f(b).unwrap_or(-1.0));
        x.partial_cmp(&y).unwrap_or(Ordering::Equal)
    };
    let ordering = match by {
        SortBy::Name => name(),
        SortBy::State | SortBy::Column(Column::Status) => view::run_state(&a.site).cmp(&view::run_state(&b.site)),
        SortBy::Column(Column::Cpu) => figure(&|p| p.stats.as_ref().and_then(|s| s.cpu_percent)),
        SortBy::Column(Column::Memory | Column::MemoryPercent) => {
            figure(&|p| p.stats.as_ref().map(|s| s.memory_bytes as f64))
        }
        SortBy::Column(Column::Disk) => figure(&|p| p.stats.as_ref().map(|s| (s.read_bytes + s.write_bytes) as f64)),
        SortBy::Column(Column::Network) => figure(&|p| p.traffic.map(|t| (t.bytes_received + t.bytes_sent) as f64)),
        // The longest-running first when ascending, as the WPF table sorts its start times.
        SortBy::Column(Column::LastStarted) => {
            figure(&|p| p.stats.as_ref().and_then(|s| s.started_ago).map(|d| -(d.as_secs_f64())))
        }
        SortBy::Column(Column::Size) => figure(&|p| p.size_bytes.map(|s| s as f64)),
        SortBy::Column(Column::Id) => a.site.id.cmp(&b.site.id),
        SortBy::Column(c) => c.text(a).to_lowercase().cmp(&c.text(b).to_lowercase()),
    };
    ordering.then_with(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_keys_round_trip() {
        for by in [SortBy::Name, SortBy::State, SortBy::Column(Column::Cpu), SortBy::Column(Column::Path)] {
            assert_eq!(SortBy::from_key(by.key()), Some(by));
        }
        assert_eq!(SortBy::from_key("Nonsense"), None);
    }

    #[test]
    fn keys_round_trip_and_defaults_are_known() {
        for c in Column::ALL {
            assert_eq!(Column::from_key(c.key()), Some(c));
        }
        for key in dnnm_application::settings::DEFAULT_PROJECT_COLUMNS {
            assert!(Column::from_key(key).is_some(), "{key}");
        }
    }
}
