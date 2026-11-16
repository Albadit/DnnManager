//! Numbers as .NET's `N0` and `N1` write them in the user's culture - the Windows separators (`locale.rs`), as the
//! WPF app does: `1.234 MB` on a Dutch Windows.

use dnnm_infrastructure::locale::number_separators;

fn separators() -> (char, char) {
    number_separators().unwrap_or(('.', ','))
}

fn group(digits: &str, thousands: char) -> String {
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(thousands);
        }
        out.push(c);
    }
    out
}

/// `{value:N0}`.
pub fn n0(value: i64) -> String {
    let (_, thousands) = separators();
    let sign = if value < 0 { "-" } else { "" };
    format!("{sign}{}", group(&value.unsigned_abs().to_string(), thousands))
}

/// `{value:N1}`.
pub fn n1(value: f64) -> String {
    let (decimal, thousands) = separators();
    let tenths = (value.abs() * 10.0).round() as u64;
    let sign = if value < 0.0 && tenths > 0 { "-" } else { "" };
    format!("{sign}{}{decimal}{}", group(&(tenths / 10).to_string(), thousands), tenths % 10)
}

/// Bytes as whole megabytes, `N0` (`bytes / 1048576d:N0` rounds; integer division where the C# divides integers).
pub fn mb(bytes: i64) -> String {
    n0(bytes / 1_048_576)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands() {
        let (decimal, thousands) = separators();
        assert_eq!(n0(1_234_567), format!("1{thousands}234{thousands}567"));
        assert_eq!(n0(12), "12");
        assert_eq!(n1(1536.25), format!("1{thousands}536{decimal}3"));
        assert_eq!(mb(5 << 20), "5");
    }
}
