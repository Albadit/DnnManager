//! Starting the programs DNN Manager reads (`ProcessRunner`): only from System32, with an argument list - never a
//! shell, never a command line built from strings - without a window, and with a time limit.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// What a program wrote and how it ended.
#[derive(Debug, Clone)]
pub struct ProcessOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl ProcessOutput {
    pub fn success(&self) -> bool {
        self.exit_code == Some(0)
    }

    /// The first line that says something - for an error message.
    pub fn message(&self) -> String {
        let text = if self.stderr.trim().is_empty() { &self.stdout } else { &self.stderr };
        text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no output").to_string()
    }
}

/// A Windows program by its path under System32 (`inetsrv\appcmd.exe`, `sc.exe`). Only administrators can change
/// that folder, which is what makes it safe to start from an elevated app (`TrustedPrograms.Resolve`).
pub fn system32(relative: &str) -> PathBuf {
    let windows = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    windows.join("System32").join(relative)
}

/// Runs a program and reads its output; ends it when it takes longer than `timeout`.
pub fn run(program: PathBuf, args: &[&str], timeout: Duration) -> Result<ProcessOutput, String> {
    let mut command = Command::new(&program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let name = program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut child = command.spawn().map_err(|e| format!("{name} can't be started: {e}"))?;

    // Read both pipes on their own threads, so a program that fills one can't stall.
    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let err = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{name} didn't answer within {} s", timeout.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(format!("{name}: {e}")),
        }
    };
    Ok(ProcessOutput {
        exit_code: status.code(),
        stdout: String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned(),
        stderr: String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned(),
    })
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn runs_a_system_program_and_reads_its_output() {
        let output = run(system32("cmd.exe"), &["/c", "echo", "hello"], Duration::from_secs(10)).unwrap();
        assert!(output.success());
        assert_eq!(output.stdout.trim(), "hello");
    }

    #[test]
    fn a_missing_program_is_an_error() {
        let err = run(system32("no-such-program.exe"), &[], Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("can't be started"));
    }

    #[test]
    fn a_program_past_its_time_is_ended() {
        let err = run(system32("PING.EXE"), &["-n", "30", "127.0.0.1"], Duration::from_millis(300)).unwrap_err();
        assert!(err.contains("didn't answer"));
    }
}
