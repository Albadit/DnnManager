//! Dates, spans and counts as the WPF app words them (.NET's "g" and "d" formats in the user's culture, `Span`,
//! `Count`, `OnOff`). Numbers come from `dnnm_application::project::view` (the user's separators).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use dnnm_application::project::view::{byte_size, group_thousands};

/// A date and time on the PC's (or SQL Server's) clock, without a time zone - what .NET's local `DateTime` is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct LocalDateTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

impl LocalDateTime {
    pub fn new(year: u16, month: u16, day: u16, hour: u16, minute: u16, second: u16) -> Self {
        Self { year, month, day, hour, minute, second }
    }

    /// Seconds since 1970-01-01 on the same clock - for differences between two of them.
    pub fn seconds(&self) -> i64 {
        // Howard Hinnant's days_from_civil.
        let (m, d) = (i64::from(self.month), i64::from(self.day));
        let y = i64::from(self.year) - i64::from(m <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        days * 86_400 + i64::from(self.hour) * 3600 + i64::from(self.minute) * 60 + i64::from(self.second)
    }

    /// A time as it reads on this PC's clock.
    pub fn from_system_time(at: SystemTime) -> Self {
        to_local(at)
    }
}

/// .NET's "g": short date and short time in the user's format ("16-11-2026 14:03").
pub fn general(at: &LocalDateTime) -> String {
    format!("{} {}", short_date(at), short_time(at))
}

#[cfg(windows)]
fn system_time(at: &LocalDateTime) -> windows::Win32::Foundation::SYSTEMTIME {
    windows::Win32::Foundation::SYSTEMTIME {
        wYear: at.year,
        wMonth: at.month,
        wDay: at.day,
        wHour: at.hour,
        wMinute: at.minute,
        wSecond: at.second,
        ..Default::default()
    }
}

/// .NET's "d": the short date in the user's format.
#[cfg(windows)]
pub fn short_date(at: &LocalDateTime) -> String {
    use windows::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx};
    use windows::core::PCWSTR;
    let mut buffer = [0u16; 80];
    // SAFETY: the user's locale (null name); the buffer's length is passed with it.
    let len = unsafe {
        GetDateFormatEx(
            PCWSTR::null(),
            DATE_SHORTDATE,
            Some(&system_time(at)),
            PCWSTR::null(),
            Some(&mut buffer),
            PCWSTR::null(),
        )
    };
    if len <= 1 {
        return format!("{:04}-{:02}-{:02}", at.year, at.month, at.day);
    }
    String::from_utf16_lossy(&buffer[..len as usize - 1])
}

#[cfg(windows)]
fn short_time(at: &LocalDateTime) -> String {
    use windows::Win32::Globalization::{GetTimeFormatEx, TIME_NOSECONDS};
    use windows::core::PCWSTR;
    let mut buffer = [0u16; 80];
    // SAFETY: as in short_date.
    let len = unsafe {
        GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&system_time(at)), PCWSTR::null(), Some(&mut buffer))
    };
    if len <= 1 {
        return format!("{:02}:{:02}", at.hour, at.minute);
    }
    String::from_utf16_lossy(&buffer[..len as usize - 1])
}

#[cfg(windows)]
fn to_local(at: SystemTime) -> LocalDateTime {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() / 100) as u64 + 116_444_736_000_000_000;
    let file = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: plain value conversions into out-parameters.
    let ok = unsafe {
        FileTimeToSystemTime(&file, &mut utc).is_ok() && SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_ok()
    };
    if !ok {
        return utc_of(at);
    }
    LocalDateTime::new(local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond)
}

#[cfg(not(windows))]
pub fn short_date(at: &LocalDateTime) -> String {
    format!("{:04}-{:02}-{:02}", at.year, at.month, at.day)
}

#[cfg(not(windows))]
fn short_time(at: &LocalDateTime) -> String {
    format!("{:02}:{:02}", at.hour, at.minute)
}

#[cfg(not(windows))]
fn to_local(at: SystemTime) -> LocalDateTime {
    utc_of(at)
}

/// The UTC date and time of `at` (civil_from_days).
fn utc_of(at: SystemTime) -> LocalDateTime {
    let secs = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    LocalDateTime::new(
        year as u16,
        month as u16,
        day as u16,
        (rest / 3600) as u16,
        (rest / 60 % 60) as u16,
        (rest % 60) as u16,
    )
}

/// "2 d 3 h", "1 h 20 min", "20 min", "45 s" (`ProjectDiagnostics.Span`).
pub fn span(span: Duration) -> String {
    let secs = span.as_secs();
    if secs >= 86_400 {
        format!("{} d {} h", secs / 86_400, secs / 3600 % 24)
    } else if secs >= 3600 {
        format!("{} h {} min", secs / 3600, secs / 60 % 60)
    } else if secs >= 60 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// .NET's `TimeSpan.ToString(@"hh\:mm")` of a time of day.
pub fn hh_mm(time: Duration) -> String {
    let secs = time.as_secs();
    format!("{:02}:{:02}", secs / 3600 % 24, secs / 60 % 60)
}

/// "1 portal", "3 aliases".
pub fn count(n: usize, one: &str, many: Option<&str>) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {}", many.map_or_else(|| format!("{one}s"), str::to_string)) }
}

pub fn on_off(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "on",
        Some(false) => "off",
        None => "-",
    }
}

/// .NET's `bool.ToString()`.
pub fn bool_text(value: bool) -> &'static str {
    if value { "True" } else { "False" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_and_counts_as_the_wpf_app() {
        assert_eq!(span(Duration::from_secs(29 * 3600)), "1 d 5 h");
        assert_eq!(span(Duration::from_secs(80 * 60)), "1 h 20 min");
        assert_eq!(span(Duration::from_secs(20 * 60)), "20 min");
        assert_eq!(span(Duration::from_secs(5)), "5 s");
        assert_eq!(count(1, "portal", None), "1 portal");
        assert_eq!(count(2, "alias", Some("aliases")), "2 aliases");
        assert_eq!(count(0, "warning", None), "0 warnings");
        assert_eq!(hh_mm(Duration::from_secs(15 * 3600 + 30 * 60)), "15:30");
    }

    #[test]
    fn dates_round_trip_and_order() {
        let at = UNIX_EPOCH + Duration::from_secs(978_307_200 + 3600 * 13 + 61);
        assert_eq!(utc_of(at), LocalDateTime::new(2001, 1, 1, 13, 1, 1));
        assert_eq!(LocalDateTime::new(2001, 1, 1, 13, 1, 1).seconds(), 978_307_200 + 3600 * 13 + 61);
        assert!(LocalDateTime::new(2024, 2, 29, 0, 0, 0) < LocalDateTime::new(2024, 3, 1, 0, 0, 0));
        let text = general(&LocalDateTime::new(2026, 11, 16, 14, 3, 0));
        assert!(text.contains("2026") && text.contains("03"), "{text}");
    }
}
