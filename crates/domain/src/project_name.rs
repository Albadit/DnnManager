//! `src/DnnManager.Domain/ProjectName.cs`: the name becomes a folder, an IIS site, a hostname and a database name.

/// The longest name a project can have.
pub const PROJECT_NAME_MAX_LENGTH: usize = 48;

/// Windows refuses files and folders with these names (with or without an extension).
const RESERVED_DEVICE_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
    "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// The project's database is named like the project, so it can't take a SQL Server system database's name.
const SQL_SYSTEM_DATABASES: &[&str] = &["master", "model", "msdb", "tempdb", "resource", "distribution"];

/// Checks a new project's name; the error is the message the user sees, word for word as the WPF app's.
pub fn validate_project_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("Project name is required.".into());
    }
    if name.chars().count() > PROJECT_NAME_MAX_LENGTH {
        return Err(format!("Project name is too long (max {PROJECT_NAME_MAX_LENGTH} characters)."));
    }
    let first = name.chars().next().unwrap_or(' ');
    if !first.is_ascii_alphanumeric() {
        return Err("Project name must start with a letter or a digit.".into());
    }
    if let Some(c) = name.chars().find(|c| !c.is_ascii_alphanumeric() && !matches!(c, '-' | '_' | '.')) {
        return Err(format!(
            "Project name contains an unsupported character '{c}'. Use letters, digits, '-', '_' or '.' only \
             (the name becomes a folder, an IIS site and a hostname)."
        ));
    }
    // '..' would climb out of the base directory; Windows strips a trailing dot, so the folder and the name would differ.
    if name.contains("..") {
        return Err("Project name cannot contain '..'.".into());
    }
    if name.ends_with('.') {
        return Err("Project name cannot end with '.'.".into());
    }
    let stem = name.split('.').next().unwrap_or(name);
    if RESERVED_DEVICE_NAMES.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return Err(format!("'{name}' uses the reserved Windows device name '{stem}'."));
    }
    if SQL_SYSTEM_DATABASES.iter().any(|d| d.eq_ignore_ascii_case(name)) {
        return Err(format!(
            "'{name}' is a SQL Server system database - the project's database is named like the project."
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        for name in ["dnn10", "My-Site_2.dev", "a", &"x".repeat(48)] {
            assert_eq!(validate_project_name(name), Ok(()), "{name}");
        }
    }

    #[test]
    fn rejects_with_the_wpf_messages() {
        let cases = [
            ("", "Project name is required."),
            ("   ", "Project name is required."),
            ("-site", "Project name must start with a letter or a digit."),
            ("a..b", "Project name cannot contain '..'."),
            ("site.", "Project name cannot end with '.'."),
            ("con", "'con' uses the reserved Windows device name 'con'."),
            ("Lpt1.dev", "'Lpt1.dev' uses the reserved Windows device name 'Lpt1'."),
            ("tempdb", "'tempdb' is a SQL Server system database - the project's database is named like the project."),
        ];
        for (name, message) in cases {
            assert_eq!(validate_project_name(name), Err(message.to_string()), "{name}");
        }
    }

    #[test]
    fn rejects_too_long_and_unsupported_characters() {
        assert!(validate_project_name(&"x".repeat(49)).unwrap_err().contains("too long (max 48"));
        assert!(validate_project_name("my site").unwrap_err().contains("unsupported character ' '"));
        assert!(validate_project_name("café").unwrap_err().contains("unsupported character 'é'"));
    }
}
