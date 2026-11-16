//! Times as the C# code formats them with `DateTime.Now` and the current culture: the day's file name
//! (`yyyyMMdd`), a line's time (`HH:mm:ss`) and a file's time in the Logs tab (`{0:g}`, the user's short date and time).

use std::time::{SystemTime, UNIX_EPOCH};

/// A moment in Windows' time zone, to the second - what `DateTime.Now` gives the C# code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LocalDateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl LocalDateTime {
    pub fn now() -> Self {
        Self::from_system_time(SystemTime::now())
    }

    pub fn from_system_time(at: SystemTime) -> Self {
        local(at).unwrap_or_else(|| utc(at))
    }

    /// `20261004` - the day in a log file's name.
    pub fn day_key(&self) -> String {
        format!("{:04}{:02}{:02}", self.year, self.month, self.day)
    }

    /// The same calendar day.
    pub fn same_day(&self, other: &Self) -> bool {
        (self.year, self.month, self.day) == (other.year, other.month, other.day)
    }

    /// `14:03:27`.
    pub fn hms(&self) -> String {
        format!("{:02}:{:02}:{:02}", self.hour, self.minute, self.second)
    }

    /// `2026-10-01 14:03:22` - an event log entry's time.
    pub fn ymd_hms(&self) -> String {
        format!("{:04}-{:02}-{:02} {}", self.year, self.month, self.day, self.hms())
    }
}

/// A Windows FILETIME (100 ns since 1601, UTC) as a `SystemTime`.
pub fn from_file_time(ticks: u64) -> SystemTime {
    const UNIX_START: u64 = 116_444_736_000_000_000;
    UNIX_EPOCH + std::time::Duration::from_nanos(ticks.saturating_sub(UNIX_START).saturating_mul(100))
}

/// .NET's `{0:g}` in the user's culture: the short date and the time without seconds ("4-10-2026 14:03").
pub fn short_date_time(at: SystemTime) -> String {
    short_date_time_windows(at).unwrap_or_else(|| {
        let t = LocalDateTime::from_system_time(at);
        format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute)
    })
}

#[cfg(windows)]
fn system_time(at: SystemTime) -> Option<windows::Win32::Foundation::SYSTEMTIME> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() / 100) as u64 + 116_444_736_000_000_000;
    let file = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: plain value conversions into out-parameters.
    unsafe {
        FileTimeToSystemTime(&file, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some(local)
}

#[cfg(windows)]
fn local(at: SystemTime) -> Option<LocalDateTime> {
    let t = system_time(at)?;
    Some(LocalDateTime {
        year: t.wYear,
        month: t.wMonth as u8,
        day: t.wDay as u8,
        hour: t.wHour as u8,
        minute: t.wMinute as u8,
        second: t.wSecond as u8,
    })
}

#[cfg(not(windows))]
fn local(_: SystemTime) -> Option<LocalDateTime> {
    None
}

#[cfg(windows)]
fn short_date_time_windows(at: SystemTime) -> Option<String> {
    use windows::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
    use windows::core::PCWSTR;
    let t = system_time(at)?;
    let (mut date, mut time) = ([0u16; 80], [0u16; 80]);
    // SAFETY: a null locale is the user's; the buffers' lengths are passed with them.
    let (d, h) = unsafe {
        (
            GetDateFormatEx(PCWSTR::null(), DATE_SHORTDATE, Some(&t), PCWSTR::null(), Some(&mut date), PCWSTR::null()),
            GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&t), PCWSTR::null(), Some(&mut time)),
        )
    };
    if d <= 1 || h <= 1 {
        return None;
    }
    Some(format!(
        "{} {}",
        String::from_utf16_lossy(&date[..d as usize - 1]),
        String::from_utf16_lossy(&time[..h as usize - 1])
    ))
}

#[cfg(not(windows))]
fn short_date_time_windows(_: SystemTime) -> Option<String> {
    None
}

/// UTC, when Windows can't say the local time.
fn utc(at: SystemTime) -> LocalDateTime {
    let secs = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as u16;
    LocalDateTime {
        year,
        month,
        day,
        hour: (rest / 3600) as u8,
        minute: (rest / 60 % 60) as u8,
        second: (rest % 60) as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn utc_fallback_is_the_calendar_day() {
        // 2026-10-04 09:30:05 UTC.
        let t = utc(UNIX_EPOCH + Duration::from_secs(1_791_106_205));
        assert_eq!((t.year, t.month, t.day, t.hms()), (2026, 10, 4, "09:30:05".to_string()));
        assert_eq!(t.day_key(), "20261004");
        assert_eq!(t.ymd_hms(), "2026-10-04 09:30:05");
    }

    #[test]
    fn file_time_is_counted_from_1601() {
        assert_eq!(from_file_time(116_444_736_000_000_000), UNIX_EPOCH);
        assert_eq!(from_file_time(116_444_736_000_000_000 + 10_000_000), UNIX_EPOCH + Duration::from_secs(1));
    }

    #[test]
    fn short_date_time_has_no_seconds() {
        let text = short_date_time(UNIX_EPOCH + Duration::from_secs(1_791_106_205));
        assert!(text.contains("2026") || text.contains("26"), "{text}");
        assert!(!text.contains(":05"), "{text}");
    }
}
