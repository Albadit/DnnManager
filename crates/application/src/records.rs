//! How DNN Manager set each project up (`ProjectRecord`, `IProjectRecords` in `Abstractions/DnnInstall.cs`): a row of
//! `dnnmanager.db`'s `projects` table, shared with the WPF app.

/// How DNN was installed (`DnnInstallMode`), as the table writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    /// DNN Manager ran DNN's unattended install: the first visit shows the new site.
    Automatic,
    /// DNN's own installation wizard, in the browser.
    Manual,
}

impl InstallMode {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallMode::Automatic => "Automatic",
            InstallMode::Manual => "Manual",
        }
    }

    /// `Enum.Parse(…, ignoreCase: true)`.
    pub fn parse(text: &str) -> Option<Self> {
        if text.eq_ignore_ascii_case("Automatic") {
            Some(InstallMode::Automatic)
        } else if text.eq_ignore_ascii_case("Manual") {
            Some(InstallMode::Manual)
        } else {
            None
        }
    }
}

/// One project's record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRecord {
    pub site: String,
    pub install_mode: InstallMode,
    /// When it was made, UTC, as .NET's round-trip format writes it ("2026-11-13T14:44:35.5371802Z").
    pub created_utc: String,
    pub dnn_version: Option<String>,
    pub host_user_name: Option<String>,
}

/// The `projects` table (`IProjectRecords`). A failure is logged and ignored, as the C# does: a record is a fact
/// about a project, never a reason for an operation to fail.
pub trait ProjectRecords: Send + Sync {
    fn find(&self, site: &str) -> Option<ProjectRecord>;
    fn save(&self, record: &ProjectRecord);
    fn remove(&self, site: &str);
}

/// Now, as .NET's `DateTime.UtcNow.ToString("O")` writes it.
pub fn utc_now_round_trip() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let ticks = now.subsec_nanos() / 100;
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil_from_days(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{ticks:07}Z", rest / 3600, rest / 60 % 60, rest % 60)
}

/// Days since 1970-01-01 to a calendar date (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_as_dotnet_writes_them() {
        assert_eq!(InstallMode::parse("automatic"), Some(InstallMode::Automatic));
        assert_eq!(InstallMode::parse(InstallMode::Manual.as_str()), Some(InstallMode::Manual));
        assert_eq!(InstallMode::parse("other"), None);
    }

    #[test]
    fn dates_are_round_trip_format() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_770), (2026, 11, 13));
        let now = utc_now_round_trip();
        assert_eq!(now.len(), "2026-11-13T14:44:35.5371802Z".len(), "{now}");
        assert!(now.ends_with('Z') && now.as_bytes()[10] == b'T');
    }
}
