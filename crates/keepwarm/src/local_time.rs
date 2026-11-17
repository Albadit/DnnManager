//! "14:02" in Windows' time zone - the `HH:mm` of the C# texts. A copy of the infrastructure's `local_time` (hours
//! and minutes), so this crate builds without it.

use std::time::{SystemTime, UNIX_EPOCH};

/// "14:02" in local time.
pub(crate) fn hm(at: SystemTime) -> String {
    let (h, m) = local_hm(at);
    format!("{h:02}:{m:02}")
}

#[cfg(windows)]
fn local_hm(at: SystemTime) -> (u16, u16) {
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
            return utc_hm(at);
        }
    }
    (local.wHour, local.wMinute)
}

#[cfg(not(windows))]
fn local_hm(at: SystemTime) -> (u16, u16) {
    utc_hm(at)
}

fn utc_hm(at: SystemTime) -> (u16, u16) {
    let secs = at.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()) % 86_400;
    ((secs / 3600) as u16, (secs / 60 % 60) as u16)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hours_and_minutes() {
        let text = super::hm(std::time::SystemTime::now());
        assert!(text.len() == 5 && text.as_bytes()[2] == b':', "{text}");
    }
}
