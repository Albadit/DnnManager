//! The user's number format from Windows' regional settings - what .NET's current culture formats with.

/// The decimal and thousands separators the user has chosen in Windows; None when Windows doesn't say.
#[cfg(windows)]
pub fn number_separators() -> Option<(char, char)> {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_SDECIMAL, LOCALE_STHOUSAND};
    use windows::core::PCWSTR;
    let read = |kind: u32| -> Option<char> {
        let mut buffer = [0u16; 8];
        // SAFETY: a null locale name is the user's default; the buffer's length is passed with it.
        let len = unsafe { GetLocaleInfoEx(PCWSTR::null(), kind, Some(&mut buffer)) };
        let text = String::from_utf16_lossy(&buffer[..(len.max(1) as usize - 1).min(buffer.len())]);
        text.chars().next()
    };
    // A group separator can be a (no-break) space; that is a char like any other.
    Some((read(LOCALE_SDECIMAL)?, read(LOCALE_STHOUSAND)?))
}

#[cfg(not(windows))]
pub fn number_separators() -> Option<(char, char)> {
    None
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn windows_names_two_different_separators() {
        let (decimal, group) = super::number_separators().unwrap();
        assert_ne!(decimal, group);
    }
}
