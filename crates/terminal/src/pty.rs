//! A program running in a Windows pseudo console (ConPTY) - ports `Infrastructure/Terminal/PseudoConsole.cs`.
//!
//! What it prints arrives on [`PseudoConsole::take_output`] as UTF-8 with VT escape sequences, and what is written to
//! [`PseudoConsole::input`] is its keyboard. This is how Windows Terminal and VS Code host a shell. Needs Windows 10
//! 1809 or later.

use std::fs::File;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Console::{COORD, ClosePseudoConsole, CreatePseudoConsole, HPCON, ResizePseudoConsole};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation, SetInformationJobObject,
    TerminateJobObject,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Threading::{
    CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, CreateProcessW, DeleteProcThreadAttributeList,
    EXTENDED_STARTUPINFO_PRESENT, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    NORMAL_PRIORITY_CLASS, PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, PROCESS_INFORMATION, ResumeThread,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, UpdateProcThreadAttribute, WaitForSingleObject,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

/// Drops the rights also when DNN Manager isn't elevated - for the tests, which run without administrator rights.
static DROP_RIGHTS_ALWAYS: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
pub(crate) fn drop_rights_always(on: bool) {
    DROP_RIGHTS_ALWAYS.store(on, Ordering::SeqCst);
}

pub struct PseudoConsole {
    console: HPCON,
    input: Option<File>,
    output: Option<File>,
    process: OwnedHandle,
    // The shell and everything it starts: closed (or ended), the job ends them all.
    job: Option<OwnedHandle>,
    process_id: u32,
    closed: bool,
}

// SAFETY: HPCON is a handle to the pseudo console; ConPTY's calls on it may be made from any thread.
unsafe impl Send for PseudoConsole {}

impl PseudoConsole {
    /// Starts `command_line` in a pseudo console of `columns` x `rows` characters - without DNN Manager's
    /// administrator rights (an unelevated token), unless `as_administrator`. Errs with
    /// [`crate::token::unavailable`]'s message when the rights can't be dropped - it never falls back to an
    /// Administrator shell by itself.
    pub fn start(
        command_line: &str,
        working_directory: &str,
        columns: u16,
        rows: u16,
        as_administrator: bool,
    ) -> Result<PseudoConsole, String> {
        let elevated = crate::token::is_elevated();
        // Elevated, an unelevated shell gets a token without the rights; not elevated ourselves (the tests), there is
        // nothing to drop.
        let token = if !as_administrator && (elevated || DROP_RIGHTS_ALWAYS.load(Ordering::SeqCst)) {
            Some(crate::token::create()?)
        } else {
            None
        };

        // SAFETY: each handle is owned as soon as it is made; the attribute list, environment and command line
        // outlive CreateProcess; the pseudo console is closed on every failure after it is made.
        unsafe {
            // Two pipes: we write the input the console reads, and read the output it writes.
            let (mut input_read, mut input_write, mut output_read, mut output_write) =
                (HANDLE::default(), HANDLE::default(), HANDLE::default(), HANDLE::default());
            CreatePipe(&mut input_read, &mut input_write, None, 0).map_err(|e| e.message())?;
            let input_read = OwnedHandle::from_raw_handle(input_read.0);
            let input_write = File::from_raw_handle(input_write.0);
            CreatePipe(&mut output_read, &mut output_write, None, 0).map_err(|e| e.message())?;
            let output_read = File::from_raw_handle(output_read.0);
            let output_write = OwnedHandle::from_raw_handle(output_write.0);

            let size = COORD { X: columns.max(2) as i16, Y: rows.max(1) as i16 };
            let console =
                CreatePseudoConsole(size, HANDLE(input_read.as_raw_handle()), HANDLE(output_write.as_raw_handle()), 0)
                    .map_err(|e| {
                        format!(
                            "Could not create a pseudo console (needs Windows 10 version 1809 or later). {}",
                            e.message()
                        )
                    })?;
            // The console has its own copies of these two ends now.
            drop(input_read);
            drop(output_write);

            match spawn(console, command_line, working_directory, as_administrator && elevated, token.as_ref()) {
                Ok((process, job, process_id)) => Ok(PseudoConsole {
                    console,
                    input: Some(input_write),
                    output: Some(output_read),
                    process,
                    job,
                    process_id,
                    closed: false,
                }),
                Err(error) => {
                    ClosePseudoConsole(console);
                    Err(error)
                }
            }
        }
    }

    /// The program's keyboard input (UTF-8); taken by the session's writer.
    pub fn take_input(&mut self) -> Option<File> {
        self.input.take()
    }

    /// What the program draws (UTF-8 with VT sequences); ends when the console is closed. Taken by the session's
    /// reader, which must keep reading while [`Self::close`] runs.
    pub fn take_output(&mut self) -> Option<File> {
        self.output.take()
    }

    pub fn process_id(&self) -> u32 {
        self.process_id
    }

    /// A handle that is signalled when the program has exited.
    pub fn process_handle(&self) -> Result<OwnedHandle, String> {
        self.process.try_clone().map_err(|e| e.to_string())
    }

    pub fn resize(&self, columns: u16, rows: u16) {
        if !self.closed {
            let size = COORD { X: columns.max(2) as i16, Y: rows.max(1) as i16 };
            // SAFETY: the console is open until close.
            let _ = unsafe { ResizePseudoConsole(self.console, size) };
        }
    }

    /// Closes the console, which ends the program in it and whatever it started. The output must still be read while
    /// this runs - closing waits for the last output to be taken - so it happens on a thread of its own.
    pub fn close(&mut self) {
        if std::mem::replace(&mut self.closed, true) {
            return;
        }
        let console = self.console.0;
        let input = self.input.take();
        let job = self.job.take();
        std::thread::spawn(move || {
            // SAFETY: the console is closed once, here; the job handle is owned.
            unsafe {
                ClosePseudoConsole(HPCON(console));
                drop(input);
                if let Some(job) = &job {
                    let _ = TerminateJobObject(HANDLE(job.as_raw_handle()), 1);
                }
            }
        });
    }
}

impl Drop for PseudoConsole {
    fn drop(&mut self) {
        self.close();
    }
}

/// Blocks until `process` has exited.
pub fn wait_for_exit(process: &OwnedHandle) {
    // SAFETY: the handle is open for the duration of the wait.
    unsafe { while WaitForSingleObject(HANDLE(process.as_raw_handle()), INFINITE) != WAIT_OBJECT_0 {} }
}

unsafe fn spawn(
    console: HPCON,
    command_line: &str,
    working_directory: &str,
    harden: bool,
    token: Option<&OwnedHandle>,
) -> Result<(OwnedHandle, Option<OwnedHandle>, u32), String> {
    // SAFETY: as in `start`; the attribute list is deleted on every path.
    unsafe {
        // The process is attached to the pseudo console through a startup attribute.
        let mut size = 0usize;
        let _ = InitializeProcThreadAttributeList(None, 1, None, &mut size);
        let mut attributes = vec![0u64; size.div_ceil(8)];
        let list = LPPROC_THREAD_ATTRIBUTE_LIST(attributes.as_mut_ptr().cast());
        InitializeProcThreadAttributeList(Some(list), 1, None, &mut size).map_err(|e| e.message())?;
        let result = (|| {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                Some(console.0 as *const _),
                std::mem::size_of::<HPCON>(),
                None,
                None,
            )
            .map_err(|e| e.message())?;

            let mut startup = STARTUPINFOEXW::default();
            startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
            // "Use these standard handles: none". Without it a program started while our own output is redirected
            // (DNN Manager launched from a script) would write there instead of to the pseudo console.
            startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
            startup.lpAttributeList = list;

            // DNN Manager's environment - for an Administrator shell without what would make it (or a program run in
            // it) load other code with those rights.
            let folders = harden.then(crate::environment::system_folders);
            let variables =
                std::env::vars_os().map(|(n, v)| (n.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()));
            let environment: Vec<u16> =
                crate::environment::environment_block(variables, folders.as_ref()).encode_utf16().collect();
            // Suspended until it is in the job, so nothing it starts escapes it.
            let flags =
                EXTENDED_STARTUPINFO_PRESENT | NORMAL_PRIORITY_CLASS | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED;
            let directory = HSTRING::from(working_directory);
            let directory = if std::path::Path::new(working_directory).is_dir() {
                PCWSTR(directory.as_ptr())
            } else {
                PCWSTR::null()
            };
            let mut command: Vec<u16> = command_line.encode_utf16().chain(Some(0)).collect();
            let mut info = PROCESS_INFORMATION::default();
            match token {
                None => CreateProcessW(
                    PCWSTR::null(),
                    Some(PWSTR(command.as_mut_ptr())),
                    None,
                    None,
                    false,
                    flags,
                    Some(environment.as_ptr().cast()),
                    directory,
                    &startup.StartupInfo,
                    &mut info,
                ),
                // A token made from DNN Manager's own (a restricted one) needs no special privilege to start a process with.
                Some(token) => CreateProcessAsUserW(
                    Some(HANDLE(token.as_raw_handle())),
                    PCWSTR::null(),
                    Some(PWSTR(command.as_mut_ptr())),
                    None,
                    None,
                    false,
                    flags,
                    Some(environment.as_ptr().cast()),
                    directory,
                    &startup.StartupInfo,
                    &mut info,
                ),
            }
            .map_err(|e| e.message())?;
            let process = OwnedHandle::from_raw_handle(info.hProcess.0);
            let thread = OwnedHandle::from_raw_handle(info.hThread.0);
            let job = new_job(&process);
            ResumeThread(HANDLE(thread.as_raw_handle()));
            Ok((process, job, info.dwProcessId))
        })();
        DeleteProcThreadAttributeList(list);
        result
    }
}

/// A job that ends the shell and what it started once closed - None when Windows won't make one (the shell then only
/// ends with its console).
fn new_job(process: &OwnedHandle) -> Option<OwnedHandle> {
    // SAFETY: the job handle is owned; the limit structure outlives the call.
    unsafe {
        let job = OwnedHandle::from_raw_handle(CreateJobObjectW(None, PCWSTR::null()).ok()?.0);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            HANDLE(job.as_raw_handle()),
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .ok()?;
        AssignProcessToJobObject(HANDLE(job.as_raw_handle()), HANDLE(process.as_raw_handle())).ok()?;
        Some(job)
    }
}
