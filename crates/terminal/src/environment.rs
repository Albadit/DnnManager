//! A shell's environment and the computer's PATH - ports `PseudoConsole.ShellEnvironment` and the parts of
//! `Infrastructure/Processes/ChildEnvironment.cs` a terminal uses (`Drop`, `MachinePath`, `MachinePathExt`).

use std::collections::BTreeMap;

/// Every variable starting so: .NET's runtime, host and SDK settings (startup hooks, profilers, diagnostic ports,
/// EventPipe output files, another runtime), NuGet's (package folders, plugins), docker's and sqlcmd's.
const DROPPED_PREFIXES: [&str; 7] = ["DOTNET_", "COMPlus_", "COR_", "CORECLR_", "NUGET_", "DOCKER_", "SQLCMD"];

/// Windows' own folders, from Windows itself rather than the user's environment.
#[derive(Debug, Clone, Default)]
pub struct SystemFolders {
    pub windows: String,
    pub system: String,
    pub program_files: String,
    pub program_files_x86: String,
    pub common_program_files: String,
    pub common_program_files_x86: String,
    pub program_data: String,
}

impl SystemFolders {
    /// The variables naming those folders.
    fn variables(&self) -> Vec<(&'static str, String)> {
        let drive = self.windows.split('\\').next().unwrap_or_default().to_string();
        vec![
            ("SystemRoot", self.windows.clone()),
            ("windir", self.windows.clone()),
            ("SystemDrive", drive),
            ("ComSpec", join(&self.system, "cmd.exe")),
            ("ProgramFiles", self.program_files.clone()),
            ("ProgramW6432", self.program_files.clone()),
            ("ProgramFiles(x86)", self.program_files_x86.clone()),
            ("CommonProgramFiles", self.common_program_files.clone()),
            ("CommonProgramW6432", self.common_program_files.clone()),
            ("CommonProgramFiles(x86)", self.common_program_files_x86.clone()),
            ("ProgramData", self.program_data.clone()),
            ("ALLUSERSPROFILE", self.program_data.clone()),
        ]
    }
}

pub fn join(folder: &str, name: &str) -> String {
    format!("{}\\{}", folder.trim_end_matches('\\'), name)
}

/// The shell's environment block: name=value strings, sorted (case-insensitively, as Windows wants them), each ended
/// by a NUL, and one more NUL at the end. For an Administrator shell (`harden` with Windows' folders, only when DNN
/// Manager is elevated) without the variables that make a program load other code, and PowerShell kept to the modules
/// of Windows and Program Files; and cmd doesn't run a program from the current folder - a site folder an app pool
/// can write to - before PATH.
pub fn environment_block(
    variables: impl IntoIterator<Item = (String, String)>,
    harden: Option<&SystemFolders>,
) -> String {
    let mut sorted: BTreeMap<String, (String, String)> = BTreeMap::new();
    let set = |sorted: &mut BTreeMap<String, (String, String)>, name: &str, value: String| {
        sorted.insert(name.to_uppercase(), (name.to_string(), value));
    };
    for (name, value) in variables {
        set(&mut sorted, &name, value);
    }
    if let Some(folders) = harden {
        sorted.retain(|key, _| !DROPPED_PREFIXES.iter().any(|prefix| key.starts_with(&prefix.to_uppercase())));
        for (name, folder) in folders.variables() {
            if !folder.is_empty() {
                set(&mut sorted, name, folder);
            }
        }
        let modules = format!(
            "{};{}",
            join(&folders.system, "WindowsPowerShell\\v1.0\\Modules"),
            join(&folders.program_files, "WindowsPowerShell\\Modules")
        );
        set(&mut sorted, "PSModulePath", modules);
    }
    set(&mut sorted, "NoDefaultCurrentDirectoryInExePath", "1".into());
    let mut block = String::new();
    for (name, value) in sorted.values() {
        if !name.is_empty() && !name.contains('=') && !name.contains('\0') && !value.contains('\0') {
            block.push_str(name);
            block.push('=');
            block.push_str(value);
            block.push('\0');
        }
    }
    block.push('\0');
    block
}

/// `value` with its %variables% filled in from `folders` or `machine` (the computer's own variables, which only
/// administrators change) - never the user's; None when one is neither.
pub fn expand(
    value: &str,
    folders: &SystemFolders,
    machine: &dyn Fn(&str) -> Option<String>,
    depth: u32,
) -> Option<String> {
    let known = folders.variables();
    let mut result = String::new();
    let mut rest = value;
    while let Some(start) = rest.find('%') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('%').filter(|&end| end > 0 && !after[..end].contains(';')) else {
            result.push_str(&rest[..start + 1]);
            rest = after;
            continue;
        };
        let name = &after[..end];
        result.push_str(&rest[..start]);
        if let Some((_, folder)) = known.iter().find(|(n, f)| n.eq_ignore_ascii_case(name) && !f.is_empty()) {
            result.push_str(folder);
        } else {
            // Neither: the whole entry is left out.
            let inner =
                (depth < 3).then(|| machine(name)).flatten().and_then(|m| expand(&m, folders, machine, depth + 1))?;
            result.push_str(&inner);
        }
        rest = &after[end + 1..];
    }
    result.push_str(rest);
    Some(result)
}

/// The computer's PATH: its %variables% filled in (see [`expand`]); a folder with a variable that can't be filled in
/// so is left out.
pub fn machine_path_from(path: &str, folders: &SystemFolders, machine: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    path.split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| expand(entry, folders, machine, 0))
        .collect()
}

mod system {
    use super::SystemFolders;
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_NOEXPAND, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
    };
    use windows::Win32::UI::Shell::{
        FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesCommon, FOLDERID_ProgramFilesCommonX86,
        FOLDERID_ProgramFilesX86, FOLDERID_System, FOLDERID_Windows, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };
    use windows::core::{GUID, HSTRING, PCWSTR, w};

    fn known(id: &GUID) -> String {
        // SAFETY: the returned string is freed with CoTaskMemFree, as SHGetKnownFolderPath asks.
        unsafe {
            match SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) {
                Ok(path) => {
                    let text = path.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(path.0 as *const _));
                    text
                }
                Err(_) => String::new(),
            }
        }
    }

    pub fn system_folders() -> SystemFolders {
        SystemFolders {
            windows: known(&FOLDERID_Windows),
            system: known(&FOLDERID_System),
            program_files: known(&FOLDERID_ProgramFiles),
            program_files_x86: known(&FOLDERID_ProgramFilesX86),
            common_program_files: known(&FOLDERID_ProgramFilesCommon),
            common_program_files_x86: known(&FOLDERID_ProgramFilesCommonX86),
            program_data: known(&FOLDERID_ProgramData),
        }
    }

    /// A variable of the computer's (HKLM), as written - its %variables% not filled in from anyone's environment.
    pub fn machine_value(name: &str) -> Option<String> {
        let key = w!("SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Environment");
        let name = HSTRING::from(name);
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND;
        // SAFETY: the first call asks for the size in bytes; the second gets a buffer of that size.
        unsafe {
            let mut size = 0u32;
            if RegGetValueW(HKEY_LOCAL_MACHINE, key, PCWSTR(name.as_ptr()), flags, None, None, Some(&mut size)).is_err()
            {
                return None;
            }
            let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
            let mut size = (buffer.len() * 2) as u32;
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key,
                PCWSTR(name.as_ptr()),
                flags,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
            .ok()
            .ok()?;
            let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
            Some(String::from_utf16_lossy(&buffer[..length]))
        }
    }
}

pub use system::{machine_value, system_folders};

/// The computer's PATH, from the registry (see [`machine_path_from`]).
pub fn machine_path() -> Vec<String> {
    machine_path_from(&machine_value("Path").unwrap_or_default(), &system_folders(), &machine_value)
}

/// The computer's PATHEXT.
pub fn machine_path_ext() -> Vec<String> {
    machine_value("PATHEXT")
        .filter(|ext| !ext.is_empty())
        .and_then(|ext| expand(&ext, &system_folders(), &machine_value, 0))
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into())
        .split(';')
        .filter(|ext| !ext.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folders() -> SystemFolders {
        SystemFolders {
            windows: "C:\\Windows".into(),
            system: "C:\\Windows\\system32".into(),
            program_files: "C:\\Program Files".into(),
            program_files_x86: "C:\\Program Files (x86)".into(),
            common_program_files: "C:\\Program Files\\Common Files".into(),
            common_program_files_x86: "C:\\Program Files (x86)\\Common Files".into(),
            program_data: "C:\\ProgramData".into(),
        }
    }

    fn vars(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect()
    }

    #[test]
    fn block_is_sorted_nul_separated_and_adds_no_current_directory() {
        let block = environment_block(vars(&[("zeta", "1"), ("Alpha", "2"), ("PATH", "C:\\x")]), None);
        assert_eq!(block, "Alpha=2\0NoDefaultCurrentDirectoryInExePath=1\0PATH=C:\\x\0zeta=1\0\0");
    }

    #[test]
    fn unelevated_shell_keeps_the_users_variables() {
        let block = environment_block(vars(&[("DOTNET_STARTUP_HOOKS", "x.dll"), ("PSModulePath", "D:\\mine")]), None);
        assert!(block.contains("DOTNET_STARTUP_HOOKS=x.dll\0"));
        assert!(block.contains("PSModulePath=D:\\mine\0"));
    }

    #[test]
    fn administrator_shell_drops_code_loading_variables_and_takes_windows_folders() {
        let block = environment_block(
            vars(&[
                ("dotnet_startup_hooks", "x.dll"),
                ("COR_PROFILER", "{x}"),
                ("DOCKER_HOST", "tcp://x"),
                ("SQLCMDPASSWORD", "x"),
                ("SystemRoot", "D:\\Evil"),
                ("PSModulePath", "D:\\mine"),
                ("Path", "C:\\Users\\me\\bin"),
            ]),
            Some(&folders()),
        );
        for gone in ["dotnet_", "COR_", "DOCKER_", "SQLCMD", "D:\\"] {
            assert!(!block.contains(gone), "{gone} in {block}");
        }
        assert!(block.contains("SystemRoot=C:\\Windows\0"));
        assert!(block.contains("ComSpec=C:\\Windows\\system32\\cmd.exe\0"));
        assert!(block.contains("SystemDrive=C:\0"));
        assert!(block.contains(
            "PSModulePath=C:\\Windows\\system32\\WindowsPowerShell\\v1.0\\Modules;C:\\Program Files\\WindowsPowerShell\\Modules\0"
        ));
        // The user's PATH stays: they type the commands.
        assert!(block.contains("Path=C:\\Users\\me\\bin\0"));
    }

    #[test]
    fn machine_path_expands_only_windows_folders_and_machine_variables() {
        let machine = |name: &str| match name {
            "JAVA_HOME" => Some("%ProgramFiles%\\Java".to_string()),
            "LOOP" => Some("%LOOP%".to_string()),
            _ => None,
        };
        let path = machine_path_from(
            "%SystemRoot%\\system32; %JAVA_HOME%\\bin;%USERPROFILE%\\bin;;C:\\Tools;%LOOP%",
            &folders(),
            &machine,
        );
        assert_eq!(path, vec!["C:\\Windows\\system32", "C:\\Program Files\\Java\\bin", "C:\\Tools"]);
    }
}
