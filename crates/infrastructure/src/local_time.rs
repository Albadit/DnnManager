//! Times as the user reads them - in Windows' time zone, which the standard library doesn't know.

use std::time::{SystemTime, UNIX_EPOCH};

/// "14:03:27" in local time.
pub fn hms(at: SystemTime) -> String {
    let (h, m, s) = local_hms(at);
    format!("{h:02}:{m:02}:{s:02}")
}

/// .NET's `{0:g}`: the user's short date and short time ("16-11-2026 14:03").
#[cfg(windows)]
pub fn short_date_time(at: SystemTime) -> String {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    use windows::core::PCWSTR;
    let ticks = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() / 100) as u64 + 116_444_736_000_000_000;
    let file = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: value conversions into out-parameters; the buffers' lengths go with them.
    unsafe {
        if FileTimeToSystemTime(&file, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return hms(at);
        }
        let mut date = [0u16; 64];
        let mut time = [0u16; 64];
        let d = GetDateFormatEx(PCWSTR::null(), DATE_SHORTDATE, Some(&local), PCWSTR::null(), Some(&mut date), PCWSTR::null());
        let t = GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&local), PCWSTR::null(), Some(&mut time));
        if d <= 1 || t <= 1 {
            return hms(at);
        }
        format!(
            "{} {}",
            String::from_utf16_lossy(&date[..d as usize - 1]),
            String::from_utf16_lossy(&time[..t as usize - 1])
        )
    }
}

#[cfg(not(windows))]
pub fn short_date_time(at: SystemTime) -> String {
    hms(at)
}

/// A time written in .NET's round-trip format ("2026-11-13T14:44:35.5371802Z") back to a time; None when it isn't one.
pub fn from_round_trip(text: &str) -> Option<SystemTime> {
    let text = text.trim().strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let (clock, fraction) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = clock.split(':').map(|p| p.parse::<u64>().ok());
    let (h, min, s) = (t.next()??, t.next()??, t.next()??);
    let digits = &fraction[..fraction.len().min(9)];
    let nanos = digits.parse::<u64>().ok()? * 10u64.pow(9 - digits.len() as u32);
    // Days since 1970 for a calendar date (Howard Hinnant's algorithm).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = u64::try_from(days).ok()? * 86_400 + h * 3600 + min * 60 + s;
    Some(UNIX_EPOCH + std::time::Duration::from_secs(secs) + std::time::Duration::from_nanos(nanos))
}

#[cfg(windows)]
fn local_hms(at: SystemTime) -> (u16, u16, u16) {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    // FILETIME counts 100 ns since 1601; Unix time starts 11 644 473 600 s later.
    let ticks = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos() / 100) as u64 + 116_444_736_000_000_000;
    let file = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: plain value conversions into out-parameters.
    unsafe {
        if FileTimeToSystemTime(&file, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).is_err()
        {
            return utc_hms(at);
        }
    }
    (local.wHour, local.wMinute, local.wSecond)
}

#[cfg(not(windows))]
fn local_hms(at: SystemTime) -> (u16, u16, u16) {
    utc_hms(at)
}

fn utc_hms(at: SystemTime) -> (u16, u16, u16) {
    let secs = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) % 86_400;
    ((secs / 3600) as u16, (secs / 60 % 60) as u16, (secs % 60) as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_round_trip_format() {
        let at = from_round_trip("1970-01-02T00:00:01.5000000Z").unwrap();
        assert_eq!(at.duration_since(UNIX_EPOCH).unwrap(), Duration::from_millis(86_401_500));
        let later = from_round_trip("2026-11-13T14:44:35.5371802Z").unwrap();
        assert_eq!(later.duration_since(UNIX_EPOCH).unwrap().as_secs(), 1_794_581_075);
        assert!(from_round_trip("yesterday").is_none());
        assert!(!short_date_time(later).is_empty());
    }
    use std::time::Duration;

    #[test]
    fn formats_two_digits_each() {
        let text = hms(UNIX_EPOCH + Duration::from_secs(3 * 3600 + 4 * 60 + 5));
        // Every time zone is whole minutes off UTC, so the seconds are the same wherever this runs.
        assert_eq!(text.len(), 8, "{text}");
        assert!(text.ends_with(":05") && text.as_bytes()[2] == b':', "{text}");
    }
}
