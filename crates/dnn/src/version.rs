//! .NET's `System.Version` as DNN Manager uses it for DNN versions: two to four numbers, a missing build or revision
//! sorting before 0 (`10.3` < `10.3.0`), and `DnnInstall.Number` (`src/DnnManager.Application/UseCases/DnnInstall.cs`).

use std::fmt;
use std::sync::LazyLock;

use regex::Regex;

/// Major and minor, and the build and revision when the version has them - compared as .NET compares them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub build: Option<u32>,
    pub revision: Option<u32>,
}

impl Version {
    pub const fn new(major: u32, minor: u32, build: u32) -> Self {
        Self { major, minor, build: Some(build), revision: None }
    }

    pub const fn new4(major: u32, minor: u32, build: u32, revision: u32) -> Self {
        Self { major, minor, build: Some(build), revision: Some(revision) }
    }

    /// `Version.TryParse`: two to four numbers between dots.
    pub fn parse(text: &str) -> Option<Self> {
        let parts: Vec<&str> = text.trim().split('.').collect();
        if !(2..=4).contains(&parts.len()) {
            return None;
        }
        let mut numbers = Vec::with_capacity(4);
        for part in parts {
            let part = part.trim();
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            numbers.push(part.parse::<u32>().ok().filter(|n| *n <= i32::MAX as u32)?);
        }
        Some(Self {
            major: numbers[0],
            minor: numbers[1],
            build: numbers.get(2).copied(),
            revision: numbers.get(3).copied(),
        })
    }

    /// The build, or 0 when there is none (`Math.Max(v.Build, 0)`).
    pub fn build_or_zero(&self) -> u32 {
        self.build.unwrap_or(0)
    }

    /// `DnnUpgradePath.Normalize`: 10.3.3 of 10.3.3.0 (or 9.3.2 of 9.3.2.24) - major, minor and build.
    pub fn normalize(&self) -> Self {
        Self::new(self.major, self.minor, self.build_or_zero())
    }

    /// `Version.ToString(fieldCount)`, missing parts written as 0 - `ToString(3)` of 7.0 is `7.0.0`.
    pub fn to_string_n(&self, fields: usize) -> String {
        let parts = [self.major, self.minor, self.build_or_zero(), self.revision.unwrap_or(0)];
        parts[..fields.clamp(1, 4)].iter().map(u32::to_string).collect::<Vec<_>>().join(".")
    }
}

impl fmt::Display for Version {
    /// `Version.ToString()`: the parts it has.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)?;
        if let Some(build) = self.build {
            write!(f, ".{build}")?;
            if let Some(revision) = self.revision {
                write!(f, ".{revision}")?;
            }
        }
        Ok(())
    }
}

/// `DnnInstall.Number`: the number of a DNN version - `10.1.0` of `10.1.0-rc1` or `v10.1.0` - or None when it has none.
pub fn number(version: &str) -> Option<Version> {
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\d+(\.\d+){1,3}").expect("valid"));
    let text = version.trim_start_matches(['v', 'V']);
    NUMBER.find(text).and_then(|m| Version::parse(m.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_as_dotnet_does() {
        assert!(Version::parse("10.3").unwrap() < Version::parse("10.3.0").unwrap());
        assert!(Version::new(9, 13, 9) < Version::new(10, 0, 0));
        assert_eq!(Version::parse("10.3.3.0").unwrap().normalize(), Version::new(10, 3, 3));
        assert_eq!(Version::parse("1.2.x"), None);
        assert_eq!(Version::parse("1"), None);
        assert_eq!(Version::new4(1, 3, 0, 0).to_string(), "1.3.0.0");
        assert_eq!(Version::parse("7.0").unwrap().to_string_n(3), "7.0.0");
    }

    #[test]
    fn the_number_of_a_dnn_version() {
        assert_eq!(number("v10.1.0-rc1"), Some(Version::new(10, 1, 0)));
        assert_eq!(number("10.1.0"), Some(Version::new(10, 1, 0)));
        assert_eq!(number("next"), None);
    }
}
