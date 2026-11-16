//! The shells a terminal offers, and the terminal's font - ports `Presentation/Services/TerminalService.cs` (and
//! `TerminalSettings`' defaults from `UserSettings.cs`).
//!
//! A terminal starts as the signed-in user - without DNN Manager's administrator rights: any shell they have, with
//! their profile and PATH. An Administrator terminal is asked for on its own; it runs with those rights, so only a
//! shell nobody else can change (one in the user's own folders, or on their PATH there, could be swapped by any
//! program of theirs) - and none of the scripts a shell runs as it starts from the user's own folders (a PowerShell
//! profile in Documents, cmd's AutoRun, ~/.bashrc), which any program of theirs could change too.

#[cfg(windows)]
use std::path::Path;

/// The font when none is chosen - and the fallback behind a chosen one.
pub const DEFAULT_FONT: &str = "Cascadia Mono, Consolas, Courier New";
/// `terminal.defaultShell`'s default.
pub const DEFAULT_SHELL: &str = "powershell";
/// `terminal.fontSize`'s default and range.
pub const DEFAULT_FONT_SIZE: f32 = 13.0;
pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 32.0;

/// Fixed-width fonts worth offering; only the installed ones are listed ([`installed_fonts`]).
pub const FONT_CANDIDATES: [&str; 10] = [
    "Cascadia Mono",
    "Cascadia Code",
    "Consolas",
    "Courier New",
    "Lucida Console",
    "JetBrains Mono",
    "Fira Code",
    "Source Code Pro",
    "Hack",
    "DejaVu Sans Mono",
];

/// The candidates among `installed` (e.g. GPUI's `cx.text_system().all_font_names()`), in the candidates' order.
pub fn installed_fonts(installed: &[String]) -> Vec<&'static str> {
    FONT_CANDIDATES.into_iter().filter(|font| installed.iter().any(|name| name.eq_ignore_ascii_case(font))).collect()
}

/// The fonts to draw with, first choice first: the setting `terminal.fontFamily` (empty for the default) followed by
/// [`DEFAULT_FONT`] - as `TerminalService.Font` builds its FontFamily.
pub fn font_families(setting: &str) -> Vec<String> {
    let list = if setting.trim().is_empty() { DEFAULT_FONT.to_string() } else { format!("{setting}, {DEFAULT_FONT}") };
    let mut families: Vec<String> = Vec::new();
    for family in list.split(',').map(str::trim).filter(|f| !f.is_empty()) {
        if !families.iter().any(|f| f.eq_ignore_ascii_case(family)) {
            families.push(family.to_string());
        }
    }
    families
}

/// A shell a terminal can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalShell {
    /// What `terminal.defaultShell` stores: "powershell", "pwsh", "cmd" or "gitbash".
    pub key: &'static str,
    pub name: &'static str,
    /// The program, e.g. `C:\Program Files\Git\bin\bash.exe`.
    pub exe_path: String,
    /// What it is started with, e.g. `-NoProfile`.
    pub arguments: &'static str,
    /// Started with DNN Manager's administrator rights - otherwise as the signed-in user, without them.
    pub as_administrator: bool,
    /// Why it isn't offered as an Administrator terminal (others could change the program) - shown greyed with this;
    /// None when it can be started.
    pub refusal: Option<String>,
}

impl TerminalShell {
    pub fn command_line(&self) -> String {
        if self.arguments.is_empty() {
            format!("\"{}\"", self.exe_path)
        } else {
            format!("\"{}\" {}", self.exe_path, self.arguments)
        }
    }

    /// The name a terminal with it gets: "Administrator: …" for one with those rights, as Windows titles its windows.
    pub fn title(&self) -> String {
        if self.as_administrator { format!("Administrator: {}", self.name) } else { self.name.to_string() }
    }

    /// Which icon it has: "powershell" (5 and 7), "cmd" or "bash".
    pub fn icon(&self) -> &'static str {
        match self.key {
            "cmd" => "cmd",
            "gitbash" => "bash",
            _ => "powershell",
        }
    }
}

/// The shells found on this PC: as the signed-in user, and for an Administrator terminal.
#[derive(Debug, Clone, Default)]
pub struct Shells {
    /// Started as the signed-in user (without administrator rights) - Command Prompt is always there. The user's own
    /// installs count (PowerShell 7 or Git for the user only, on their PATH).
    pub user: Vec<TerminalShell>,
    /// Each one found, those others could change greyed with their refusal - Command Prompt is always one that can be
    /// started.
    pub administrator: Vec<TerminalShell>,
}

impl Shells {
    /// The shell `default_key` (the setting `terminal.defaultShell`) names, or the first installed one when that one
    /// isn't (any more).
    pub fn default_shell(&self, default_key: &str) -> Option<&TerminalShell> {
        self.user.iter().find(|s| s.key.eq_ignore_ascii_case(default_key)).or(self.user.first())
    }

    /// The default shell for an Administrator terminal - the settings' one when it may be, otherwise Command Prompt.
    pub fn default_administrator_shell(&self, default_key: &str) -> Option<&TerminalShell> {
        self.administrator
            .iter()
            .find(|s| s.refusal.is_none() && s.key.eq_ignore_ascii_case(default_key))
            .or_else(|| self.administrator.iter().find(|s| s.refusal.is_none()))
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    /// Builds the lists from candidate paths - `exists` and `resolve` (whether a copy may run as Administrator) are
    /// the system's in [`find_shells`], stand-ins in the tests.
    fn add(
        &mut self,
        (key, name): (&'static str, &'static str),
        user_candidates: &[String],
        admin_candidates: &[String],
        (user_arguments, admin_arguments): (&'static str, &'static str),
        exists: &dyn Fn(&str) -> bool,
        resolve: &dyn Fn(&str, &str) -> Result<String, String>,
    ) {
        if let Some(exe) = user_candidates.iter().find(|c| exists(c)) {
            self.user.push(shell(key, name, exe.clone(), user_arguments, false, None));
        }
        // The first copy that may run as Administrator - or, when none may, the first found, greyed with why.
        let mut refused = None;
        for candidate in admin_candidates.iter().filter(|c| exists(c)) {
            match resolve(candidate, name) {
                Ok(path) => {
                    self.administrator.push(shell(key, name, path, admin_arguments, true, None));
                    return;
                }
                Err(problem) => {
                    refused.get_or_insert_with(|| {
                        shell(key, name, candidate.clone(), admin_arguments, true, Some(problem))
                    });
                }
            }
        }
        if let Some(refused) = refused {
            self.administrator.push(refused);
        }
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
fn shell(
    key: &'static str,
    name: &'static str,
    exe_path: String,
    arguments: &'static str,
    as_administrator: bool,
    refusal: Option<String>,
) -> TerminalShell {
    TerminalShell { key, name, exe_path, arguments, as_administrator, refusal }
}

/// Where the shells are looked for.
#[cfg(windows)]
struct Places {
    system: String,
    program_files: String,
    program_files_x86: String,
    local_app_data: String,
    user_path: Vec<String>,
    machine_path_pwsh: Vec<String>,
}

#[cfg(windows)]
fn build(
    places: &Places,
    exists: &dyn Fn(&str) -> bool,
    resolve: &dyn Fn(&str, &str) -> Result<String, String>,
) -> Shells {
    use crate::environment::join;
    let mut shells = Shells::default();

    let windows_power_shell = join(&places.system, "WindowsPowerShell\\v1.0\\powershell.exe");
    let only = std::slice::from_ref(&windows_power_shell);
    shells.add(("powershell", "PowerShell"), only, only, ("", "-NoProfile"), exists, resolve);

    let all_users_pwsh = join(&places.program_files, "PowerShell\\7\\pwsh.exe");
    // As the user: theirs first (on their own PATH - a per-user or Store install), then the one for all users.
    let mut user_pwsh: Vec<String> = places.user_path.iter().map(|folder| join(folder, "pwsh.exe")).collect();
    user_pwsh.push(all_users_pwsh.clone());
    // As Administrator: on the computer's PATH, then Program Files - only a copy administrators alone can change.
    let mut admin_pwsh = places.machine_path_pwsh.clone();
    admin_pwsh.push(all_users_pwsh);
    shells.add(("pwsh", "PowerShell 7"), &user_pwsh, &admin_pwsh, ("", "-NoProfile"), exists, resolve);

    // Windows' own, whatever %ComSpec% says; /d as Administrator: no AutoRun command from the registry.
    let cmd = join(&places.system, "cmd.exe");
    let only = std::slice::from_ref(&cmd);
    shells.add(("cmd", "Command Prompt"), only, only, ("", "/d"), exists, resolve);

    // Git for Windows: for all users, the 32-bit one, or installed for this user only.
    let bash = [
        join(&places.program_files, "Git\\bin\\bash.exe"),
        join(&places.program_files_x86, "Git\\bin\\bash.exe"),
        join(&places.local_app_data, "Programs\\Git\\bin\\bash.exe"),
    ];
    shells.add(("gitbash", "Git Bash"), &bash, &bash, ("--login -i", "--noprofile --norc -i"), exists, resolve);
    shells
}

/// No shell off Windows: the terminal runs them in ConPTY.
#[cfg(not(windows))]
pub fn find_shells() -> Shells {
    Shells::default()
}

/// The shells installed on this PC. Reads the disk (and, elevated, who may change each program): call it off the UI
/// thread, once.
#[cfg(windows)]
pub fn find_shells() -> Shells {
    let folders = crate::environment::system_folders();
    // The user's PATH as this process has it (the computer's and theirs): for a shell that runs as them.
    let user_path = std::env::var("PATH")
        .unwrap_or_default()
        .split(';')
        .map(str::trim)
        .filter(|folder| crate::trust::is_fully_qualified(folder))
        .map(str::to_string)
        .collect();
    let places = Places {
        system: folders.system.clone(),
        program_files: folders.program_files.clone(),
        program_files_x86: folders.program_files_x86.clone(),
        local_app_data: std::env::var("LOCALAPPDATA").unwrap_or_default(),
        user_path,
        machine_path_pwsh: crate::trust::on_machine_path("pwsh.exe"),
    };
    let elevated = crate::token::is_elevated();
    let exists = |path: &str| Path::new(path).is_file();
    let resolve = |path: &str, name: &str| {
        let resolved = crate::trust::resolve(path, elevated);
        resolved.path.clone().ok_or_else(|| resolved.problem(name))
    };
    build(&places, &exists, &resolve)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn places() -> Places {
        Places {
            system: "C:\\Windows\\system32".into(),
            program_files: "C:\\Program Files".into(),
            program_files_x86: "C:\\Program Files (x86)".into(),
            local_app_data: "C:\\Users\\me\\AppData\\Local".into(),
            user_path: vec!["C:\\Users\\me\\AppData\\Local\\Microsoft\\WindowsApps".into()],
            machine_path_pwsh: vec![],
        }
    }

    #[test]
    fn finds_the_installed_shells_with_their_arguments() {
        let installed = [
            "C:\\Windows\\system32\\WindowsPowerShell\\v1.0\\powershell.exe",
            "C:\\Windows\\system32\\cmd.exe",
            "C:\\Users\\me\\AppData\\Local\\Microsoft\\WindowsApps\\pwsh.exe",
            "C:\\Users\\me\\AppData\\Local\\Programs\\Git\\bin\\bash.exe",
        ];
        let exists = |p: &str| installed.contains(&p);
        // Only Windows' own folder is admin-only here.
        let resolve = |p: &str, _: &str| {
            if p.starts_with("C:\\Windows\\") { Ok(p.to_string()) } else { Err(format!("refused {p}")) }
        };
        let shells = build(&places(), &exists, &resolve);
        let user: Vec<_> = shells.user.iter().map(|s| (s.key, s.command_line())).collect();
        assert_eq!(
            user,
            vec![
                ("powershell", "\"C:\\Windows\\system32\\WindowsPowerShell\\v1.0\\powershell.exe\"".to_string()),
                ("pwsh", "\"C:\\Users\\me\\AppData\\Local\\Microsoft\\WindowsApps\\pwsh.exe\"".into()),
                ("cmd", "\"C:\\Windows\\system32\\cmd.exe\"".into()),
                ("gitbash", "\"C:\\Users\\me\\AppData\\Local\\Programs\\Git\\bin\\bash.exe\" --login -i".into()),
            ]
        );
        // The per-user pwsh isn't an admin candidate at all; the per-user bash is, refused.
        let admin: Vec<_> = shells.administrator.iter().map(|s| (s.key, s.command_line(), s.refusal.clone())).collect();
        assert_eq!(
            admin,
            vec![
                (
                    "powershell",
                    "\"C:\\Windows\\system32\\WindowsPowerShell\\v1.0\\powershell.exe\" -NoProfile".to_string(),
                    None
                ),
                ("cmd", "\"C:\\Windows\\system32\\cmd.exe\" /d".into(), None),
                (
                    "gitbash",
                    "\"C:\\Users\\me\\AppData\\Local\\Programs\\Git\\bin\\bash.exe\" --noprofile --norc -i".into(),
                    Some("refused C:\\Users\\me\\AppData\\Local\\Programs\\Git\\bin\\bash.exe".into())
                ),
            ]
        );
        assert!(shells.administrator.iter().all(|s| s.as_administrator));
        assert_eq!(shells.administrator[0].title(), "Administrator: PowerShell");
    }

    #[test]
    fn default_shells() {
        let exists = |p: &str| p.ends_with("cmd.exe") || p.ends_with("bash.exe");
        let resolve = |p: &str, _: &str| if p.ends_with("cmd.exe") { Ok(p.to_string()) } else { Err("no".to_string()) };
        let shells = build(&places(), &exists, &resolve);
        assert_eq!(shells.default_shell("gitbash").unwrap().key, "gitbash");
        assert_eq!(shells.default_shell("GITBASH").unwrap().key, "gitbash");
        // PowerShell isn't there: the first installed one.
        assert_eq!(shells.default_shell("powershell").unwrap().key, "cmd");
        // Git Bash may not run as Administrator: Command Prompt.
        assert_eq!(shells.default_administrator_shell("gitbash").unwrap().key, "cmd");
    }

    #[test]
    fn names_and_icons() {
        let s = shell("gitbash", "Git Bash", "C:\\b.exe".into(), "", false, None);
        assert_eq!((s.title(), s.icon(), s.command_line()), ("Git Bash".into(), "bash", "\"C:\\b.exe\"".into()));
    }

    #[test]
    fn fonts() {
        assert_eq!(font_families(""), vec!["Cascadia Mono", "Consolas", "Courier New"]);
        assert_eq!(font_families("Consolas"), vec!["Consolas", "Cascadia Mono", "Courier New"]);
        let installed = vec!["consolas".to_string(), "Arial".into(), "Hack".into()];
        assert_eq!(installed_fonts(&installed), vec!["Consolas", "Hack"]);
    }

    #[test]
    fn finds_command_prompt_on_this_pc() {
        let shells = find_shells();
        assert!(shells.user.iter().any(|s| s.key == "cmd"));
        assert!(shells.default_administrator_shell("cmd").is_some());
    }
}
