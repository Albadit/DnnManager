//! IIS through its own command line, `%windir%\System32\inetsrv\appcmd.exe` - the WPF app uses
//! Microsoft.Web.Administration, which has no Rust counterpart (`IisManager.cs`). Reading IIS's configuration needs
//! administrator rights, as it does for the WPF app. Why appcmd for each write: [`crate::iis_admin`].

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dnnm_application::iis::{
    IisBinding, IisError, IisManager, IisPoolSettings, IisServerAction, IisServerState, IisSiteDetails, IisSiteRuntime,
    states,
};
use dnnm_application::project::SiteTraffic;

use crate::iis_admin::{self, SiteEntry};
use crate::process::{ProcessOutput, run, system32};
use crate::{certificates, iis_counters, web_config};

/// A quick question to IIS; a change (start, stop) gets longer.
const READ_LIMIT: Duration = Duration::from_secs(15);
const CHANGE_LIMIT: Duration = Duration::from_secs(60);
/// How long StartSite waits for an app pool that is still stopping.
const POOL_STOP_WAIT: Duration = Duration::from_secs(20);
/// How long RemoveSite and RenameSite wait for a worker process before ending it.
const WORKER_EXIT_WAIT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(250);

const STILL_STOPPING: &str =
    "Its app pool is still stopping (the worker process hasn't ended yet) - try again in a moment.";
const LINK_HINT: &str = "Hint: if the link is meant to be there, recreate it from an administrator command prompt (mklink /J) - or copy the files in instead of linking them.";

/// `IisManager` over appcmd.exe, sc.exe, iisreset.exe and icacls.exe.
#[derive(Debug, Default)]
pub struct AppcmdIis;

fn not_found(site: &str) -> IisError {
    IisError(format!("Site '{site}' not found"))
}

fn find<'a>(sites: &'a [SiteEntry], name: &str) -> Option<&'a SiteEntry> {
    sites.iter().find(|s| s.name.eq_ignore_ascii_case(name))
}

fn lookup<'a, V>(map: &'a HashMap<String, V>, key: &str) -> Option<&'a V> {
    map.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v)
}

impl AppcmdIis {
    fn appcmd() -> PathBuf {
        system32(r"inetsrv\appcmd.exe")
    }

    fn appcmd_run(args: &[&str], limit: Duration) -> Result<ProcessOutput, IisError> {
        run(Self::appcmd(), args, limit).map_err(IisError)
    }

    /// What appcmd printed for a read; its error when it failed.
    fn read(args: &[&str]) -> Result<String, IisError> {
        let output = Self::appcmd_run(args, READ_LIMIT)?;
        if output.success() { Ok(output.stdout) } else { Err(IisError(access_message(&output))) }
    }

    fn change(args: &[&str]) -> Result<(), IisError> {
        let output = Self::appcmd_run(args, CHANGE_LIMIT)?;
        // appcmd says "already started/stopped" with an error code; that is the state we wanted.
        let text = format!("{}{}", output.stdout, output.stderr).to_ascii_lowercase();
        if output.success() || text.contains("already") { Ok(()) } else { Err(IisError(access_message(&output))) }
    }

    fn change_list(args: &[String]) -> Result<(), IisError> {
        Self::change(&args.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// The configuration section `system.applicationHost/<name>` as applicationHost.config has it.
    fn section(name: &str) -> Result<String, IisError> {
        Self::read(&["list", "config", &format!("/section:system.applicationHost/{name}")])
    }

    fn sites() -> Result<Vec<SiteEntry>, IisError> {
        iis_admin::parse_site_entries(&Self::read(&["list", "site", "/config", "/xml"])?).map_err(IisError)
    }

    fn pool_states() -> Result<HashMap<String, String>, IisError> {
        Ok(parse_app_pools(&Self::read(&["list", "apppool", "/xml"])?))
    }

    fn pool_state(pool: &str) -> Option<String> {
        Self::pool_states().ok().and_then(|states| lookup(&states, pool).cloned())
    }

    /// The pool's worker processes; None when they can't be read (IIS stopped, no rights).
    fn workers_of(pool: &str) -> Option<Vec<u32>> {
        let xml = Self::read(&["list", "wp", "/xml"]).ok()?;
        Some(lookup(&parse_worker_processes(&xml), pool).cloned().unwrap_or_default())
    }

    /// An app pool that is still stopping - its worker process on its way out, as right after a stop - can't be
    /// started: IIS refuses. Waits for it to have stopped; false when it still hasn't after a while.
    fn wait_while_pool_stopping(pool: Option<&str>) -> bool {
        let Some(pool) = pool else { return true };
        let deadline = Instant::now() + POOL_STOP_WAIT;
        while Self::pool_state(pool).as_deref() == Some(states::STOPPING) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(POLL);
        }
        true
    }

    /// Polls until the pool is stopped with no worker process left - its file handles released. A worker that hasn't
    /// ended by `timeout` is ended by force, so the site's files can be deleted.
    fn wait_for_pool_to_stop(&self, pool: &str, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            // IIS's state momentarily unavailable: asked again.
            if let Ok(pools) = Self::pool_states() {
                match lookup(&pools, pool) {
                    None => return,
                    Some(state) if state == states::STOPPED => match Self::workers_of(pool) {
                        // Only an observed zero counts: a failed read isn't "no workers, safe to delete".
                        Some(workers) if workers.is_empty() => return,
                        Some(_) => {}
                        // With the web service stopped there are no worker processes to read - nor any left.
                        None => {
                            if matches!(self.server_state(), IisServerState::Stopped | IisServerState::NotInstalled) {
                                return;
                            }
                        }
                    },
                    Some(_) => {}
                }
            }
            std::thread::sleep(POLL);
        }
        log::warn!("App pool '{pool}' did not stop within the time limit; ending its worker processes.");
        for pid in Self::workers_of(pool).unwrap_or_default() {
            if !end_process(pid) {
                log::warn!("Could not end w3wp {pid} of app pool {pool}");
            }
        }
    }
}

impl IisManager for AppcmdIis {
    fn is_available(&self) -> bool {
        Self::appcmd().is_file()
    }

    fn server_state(&self) -> IisServerState {
        if !self.is_available() {
            return IisServerState::NotInstalled;
        }
        match run(system32("sc.exe"), &["query", "W3SVC"], READ_LIMIT) {
            Ok(output) => parse_service_state(&output.stdout),
            Err(_) => IisServerState::Unknown,
        }
    }

    fn site_runtimes(&self) -> Result<BTreeMap<String, IisSiteRuntime>, IisError> {
        let sites = Self::appcmd_run(&["list", "site", "/config", "/xml"], READ_LIMIT)?;
        if !sites.success() {
            return Err(IisError(access_message(&sites)));
        }
        // App pool states and worker processes: a failure here leaves them unknown, not the sites unreadable.
        let pools = Self::pool_states().unwrap_or_default();
        let workers = Self::read(&["list", "wp", "/xml"]).map(|xml| parse_worker_processes(&xml)).unwrap_or_default();
        parse_sites(&sites.stdout, &pools, &workers).map_err(IisError)
    }

    fn start_site(&self, site: &str) -> Result<(), IisError> {
        let sites = Self::sites()?;
        let entry = find(&sites, site).ok_or_else(|| not_found(site))?;
        if !Self::wait_while_pool_stopping(entry.root_pool()) {
            return Err(IisError(STILL_STOPPING.into()));
        }
        // A stopped pool (StopSite stops it too) would leave the started site answering 503.
        if let Some(pool) = entry.root_pool()
            && Self::pool_state(pool).as_deref() == Some(states::STOPPED)
        {
            Self::change_list(&iis_admin::object_args("start", "apppool", pool))?;
        }
        if !matches!(entry.state.as_str(), states::STARTED | states::STARTING) {
            Self::change_list(&iis_admin::object_args("start", "site", &entry.name))?;
        }
        Ok(())
    }

    fn stop_site(&self, site: &str) -> Result<(), IisError> {
        let sites = Self::sites()?;
        let entry = find(&sites, site).ok_or_else(|| not_found(site))?;
        if !matches!(entry.state.as_str(), states::STOPPED | states::STOPPING) {
            Self::change_list(&iis_admin::object_args("stop", "site", &entry.name))?;
        }
        // Stopping the pool ends its worker process (and the memory it holds) - unless another site shares it.
        if let Some(pool) = entry.root_pool()
            && !iis_admin::pool_used_by_others(&sites, &entry.name, pool)
            && !matches!(Self::pool_state(pool).as_deref(), Some(states::STOPPED | states::STOPPING) | None)
        {
            Self::change_list(&iis_admin::object_args("stop", "apppool", pool))?;
        }
        Ok(())
    }

    fn stop_site_and_wait(&self, site: &str, timeout: Duration) -> Result<(), IisError> {
        self.stop_site(site)?;
        let sites = Self::sites()?;
        let entry = find(&sites, site).ok_or_else(|| not_found(site))?;
        // A pool another site shares keeps running (stop_site leaves it): its worker process isn't this site's to end.
        let Some(pool) = entry.root_pool().filter(|p| !iis_admin::pool_used_by_others(&sites, &entry.name, p)) else {
            return Ok(());
        };
        // Stopping, IIS gives the worker process its shutdown time limit (90 seconds by default) to finish - and the
        // pool reads Stopped only once it has gone. Ended by force when it takes longer.
        self.wait_for_pool_to_stop(pool, timeout);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if Self::workers_of(pool).is_some_and(|w| w.is_empty()) {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(IisError(format!(
                    "The worker process of app pool '{pool}' still runs - it couldn't be ended (DNN Manager may need to run as administrator)."
                )));
            }
            std::thread::sleep(POLL);
        }
    }

    fn restart_site(&self, site: &str) -> Result<(), IisError> {
        let sites = Self::sites()?;
        let entry = find(&sites, site).ok_or_else(|| not_found(site))?;
        // Right after a stop the pool may still be stopping: neither recycled nor started then - waited for.
        if !Self::wait_while_pool_stopping(entry.root_pool()) {
            return Err(IisError(STILL_STOPPING.into()));
        }
        if let Some(pool) = entry.root_pool() {
            match Self::pool_state(pool).as_deref() {
                Some(states::STARTED) => Self::change_list(&iis_admin::object_args("recycle", "apppool", pool))?,
                Some(states::STOPPED) => Self::change_list(&iis_admin::object_args("start", "apppool", pool))?,
                _ => {}
            }
        }
        if !matches!(entry.state.as_str(), states::STARTED | states::STARTING) {
            Self::change_list(&iis_admin::object_args("start", "site", &entry.name))?;
        }
        Ok(())
    }

    fn recycle_app_pool(&self, site: &str) -> Result<(), IisError> {
        let sites = Self::sites()?;
        let entry = find(&sites, site).ok_or_else(|| not_found(site))?;
        match entry.root_pool() {
            Some(pool) if Self::pool_state(pool).as_deref() == Some(states::STARTED) => {
                Self::change_list(&iis_admin::object_args("recycle", "apppool", pool))
            }
            _ => Ok(()),
        }
    }

    fn control_server(&self, action: IisServerAction) -> Result<(), IisError> {
        let flag = match action {
            IisServerAction::Start => "/start",
            IisServerAction::Stop => "/stop",
            IisServerAction::Restart => "/restart",
        };
        let output = run(system32("iisreset.exe"), &[flag], Duration::from_secs(120)).map_err(IisError)?;
        if output.success() { Ok(()) } else { Err(IisError(output.message())) }
    }

    fn create_site(&self, site: &str, physical_path: &Path, host: &str, port: u16) -> Result<(), IisError> {
        // Fully tear down any existing site/pool of this name first - stopping and waiting for its worker to exit -
        // so it is never re-created on top of a running w3wp that still holds the folder's files. As the C#, a
        // failure there shows in what follows.
        let _ = self.remove_site(site);
        // A pool of this name that other sites use too stays (remove_site leaves it): the new site joins it, as it is.
        if lookup(&Self::pool_states()?, site).is_none() {
            Self::change_list(&iis_admin::add_pool_args(site))?;
        }
        Self::change_list(&iis_admin::add_site_args(site, &physical_path.to_string_lossy(), host, port))?;
        Self::change_list(&iis_admin::site_pool_args(site, site))
    }

    fn remove_site(&self, site: &str) -> Result<(), IisError> {
        // 1) Stop the site and pool, but don't remove them yet: the worker lingers (up to its shutdown time limit)
        //    and keeps file handles open on the site's folder - removing the pool now would orphan it and let the
        //    caller's delete of the folder race it. A pool named like the site that other sites use too is theirs as
        //    well: it is left running and in IIS.
        let sites = Self::sites()?;
        let shared = iis_admin::pool_used_by_others(&sites, site, site);
        let entry = find(&sites, site);
        if let Some(entry) = entry
            && entry.state != states::STOPPED
        {
            let _ = Self::change_list(&iis_admin::object_args("stop", "site", &entry.name));
        }
        let pools = Self::pool_states()?;
        let pool = if shared { None } else { lookup(&pools, site) };
        if pool.is_some_and(|state| state != states::STOPPED) {
            let _ = Self::change_list(&iis_admin::object_args("stop", "apppool", site));
        }
        // 2) Wait for the worker process to end, so it releases its files - ended by force when it doesn't in time.
        if pool.is_some() {
            self.wait_for_pool_to_stop(site, WORKER_EXIT_WAIT);
        }
        // 3) Now the stopped site and pool can go.
        if let Some(entry) = entry {
            Self::change_list(&iis_admin::object_args("delete", "site", &entry.name))?;
        }
        if pool.is_some() {
            Self::change_list(&iis_admin::object_args("delete", "apppool", site))?;
        }
        Ok(())
    }

    fn log_directory(&self, site: &str) -> Option<PathBuf> {
        match Self::section("sites") {
            Ok(xml) => iis_admin::log_directory_from(&xml, site),
            Err(e) => {
                log::warn!("Could not read the log folder of IIS site {site}: {e}");
                None
            }
        }
    }

    fn app_pool_identity(&self, site: &str) -> String {
        let read = || -> Result<String, IisError> {
            let config = iis_admin::site_config_from(&Self::section("sites")?, site);
            let pool = config.and_then(|c| c.root_pool).unwrap_or_else(|| site.to_string());
            let account = iis_admin::pool_details_from(&Self::section("applicationPools")?, &pool)
                .map_or_else(|| format!(r"IIS APPPOOL\{pool}"), |p| p.account);
            Ok(account)
        };
        read().unwrap_or_else(|e| {
            // Every site DNN Manager creates runs as its own pool's identity.
            log::warn!("Could not read the identity of IIS site {site}: {e}");
            format!(r"IIS APPPOOL\{site}")
        })
    }

    fn enable_user_profile(&self, site: &str) -> Result<(), IisError> {
        let sites = Self::sites()?;
        let pools = Self::pool_states()?;
        let pool = match find(&sites, site) {
            Some(entry) => entry.root_pool().map(str::to_string),
            None => lookup(&pools, site).map(|_| site.to_string()),
        }
        .filter(|p| lookup(&pools, p).is_some())
        .ok_or_else(|| IisError(format!("IIS site '{site}' has no app pool.")))?;
        Self::change_list(&iis_admin::user_profile_args(&pool))
    }

    fn site_traffic(&self) -> HashMap<String, SiteTraffic> {
        iis_counters::web_service().traffic()
    }

    fn requests_served(&self) -> HashMap<String, u64> {
        iis_counters::web_service().requests
    }

    fn site_details(&self, site: &str) -> Option<IisSiteDetails> {
        let read = || -> Result<Option<IisSiteDetails>, IisError> {
            let Some(config) = iis_admin::site_config_from(&Self::section("sites")?, site) else { return Ok(None) };
            let runtimes = self.site_runtimes()?;
            let runtime = runtimes.iter().find(|(name, _)| name.eq_ignore_ascii_case(&config.name)).map(|(_, r)| r);
            let pool = match &config.root_pool {
                Some(name) => iis_admin::pool_details_from(&Self::section("applicationPools")?, name).map(|mut p| {
                    p.state = Self::pool_state(&p.name);
                    p.worker_process_ids = Self::workers_of(&p.name).unwrap_or_default();
                    p
                }),
                None => None,
            };
            let mut bindings = config.bindings;
            certificates::complete(&mut bindings);
            Ok(Some(IisSiteDetails {
                id: config.id,
                state: runtime.map_or_else(|| states::UNKNOWN.to_string(), |r| r.state.clone()),
                physical_path: config.physical_path,
                config_path: format!("MACHINE/WEBROOT/APPHOST/{}", config.name),
                server_auto_start: config.server_auto_start,
                preload_enabled: config.preload_enabled,
                bindings,
                pool,
                name: config.name,
            }))
        };
        read().unwrap_or_else(|e| {
            log::warn!("Could not read the details of IIS site {site}: {e}");
            None
        })
    }

    fn replace_http_bindings(&self, site: &str, bindings: &[(String, u16)]) -> Result<(), IisError> {
        let config = iis_admin::site_config_from(&Self::section("sites")?, site).ok_or_else(|| not_found(site))?;
        Self::change_list(&iis_admin::replace_bindings_args(&config.name, &config.bindings, bindings))
    }

    fn set_pool_settings(&self, site: &str, settings: &IisPoolSettings) -> Result<(), IisError> {
        let sites = Self::sites()?;
        let pools = Self::pool_states()?;
        let no_pool = || IisError(format!("IIS site '{site}' has no app pool."));
        let entry = find(&sites, site).ok_or_else(no_pool)?;
        let pool = entry.root_pool().filter(|p| lookup(&pools, p).is_some()).ok_or_else(no_pool)?;
        if iis_admin::pool_used_by_others(&sites, &entry.name, pool) {
            return Err(IisError(format!(
                "The app pool '{pool}' is used by other sites too - change it in IIS Manager."
            )));
        }
        Self::change_list(&iis_admin::pool_settings_args(pool, settings).map_err(IisError)?)
    }

    fn rename_site(&self, site: &str, new_name: &str, physical_path: &Path) -> Result<(), IisError> {
        // Stopped first, its worker gone: the pool can't be renamed while a worker runs as its identity, and the
        // caller may move the folder the worker holds files in.
        let sites = Self::sites()?;
        let entry = find(&sites, site).ok_or_else(|| not_found(site))?.clone();
        // Only capitals changed: the name IIS finds is this site's own.
        let case_only = new_name.eq_ignore_ascii_case(site);
        if !case_only && find(&sites, new_name).is_some() {
            return Err(IisError(format!("IIS already has a site named '{new_name}'.")));
        }
        let pools = Self::pool_states()?;
        let pool = entry.root_pool().filter(|p| lookup(&pools, p).is_some());
        // Its own pool is stopped, whatever it is called: its worker holds files in the folder the caller moves.
        let stop_pool = pool.filter(|p| !iis_admin::pool_used_by_others(&sites, &entry.name, p));
        let rename_pool = stop_pool.is_some_and(|p| p.eq_ignore_ascii_case(site))
            && (case_only || lookup(&pools, new_name).is_none());
        if entry.state != states::STOPPED {
            let _ = Self::change_list(&iis_admin::object_args("stop", "site", &entry.name));
        }
        if let Some(pool) = stop_pool {
            if lookup(&pools, pool).is_some_and(|s| s != states::STOPPED) {
                let _ = Self::change_list(&iis_admin::object_args("stop", "apppool", pool));
            }
            self.wait_for_pool_to_stop(pool, WORKER_EXIT_WAIT);
        }
        for command in iis_admin::rename_args(&entry, new_name, rename_pool, &physical_path.to_string_lossy()) {
            Self::change_list(&command)?;
        }
        Ok(())
    }

    fn grant_permissions(&self, path: &Path, identities: &[String]) -> Result<(), IisError> {
        let full = std::path::absolute(path).map_err(|e| IisError(e.to_string()))?;
        // Rights given to a link - the folder itself or one on the way to it - would go to what it points to: another
        // folder the app pool could then change.
        let root = full.ancestors().last().unwrap_or(&full).to_path_buf();
        if web_config::has_link(&root, &full) {
            return Err(IisError(format!(
                "{} is a link or junction, or reached through one - IIS's rights are given to a site's own folder, not to what a link points to.\n{LINK_HINT}",
                path.display()
            )));
        }
        if !full.is_dir() {
            return Err(IisError(format!("Could not find a part of the path '{}'.", path.display())));
        }
        let target = full.to_string_lossy();
        for identity in identities {
            // An identity that can't be given its rights is skipped, as the C# skips it: the others still get theirs.
            for command in iis_admin::icacls_args(&target, identity) {
                let args: Vec<&str> = command.iter().map(String::as_str).collect();
                match run(system32("icacls.exe"), &args, CHANGE_LIMIT) {
                    Ok(output) if output.success() => {}
                    Ok(output) => log::warn!("Skip identity {identity}: {}", output.message()),
                    Err(e) => log::warn!("Skip identity {identity}: {e}"),
                }
            }
        }
        Ok(())
    }

    fn remove_app_pool_profile(&self, pool: &str) -> Result<(), IisError> {
        // Never a built-in pool's shared profile; and a pool still in IIS (another site uses it) still runs as it.
        if iis_admin::RESERVED_APP_POOLS.iter().any(|r| r.eq_ignore_ascii_case(pool))
            || Self::pool_states().is_ok_and(|pools| lookup(&pools, pool).is_some())
        {
            return Ok(());
        }
        delete_profile(&iis_admin::app_pool_sid(pool)).map_err(IisError)
    }
}

/// Deletes the profile of the account `sid` - its folder and its ProfileList entry. Ok when there is none.
/// The C# asks PowerShell (`Win32_UserProfile` … `Remove-CimInstance`); `DeleteProfileW` is the API under it.
#[cfg(windows)]
fn delete_profile(sid: &str) -> Result<(), String> {
    use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
    use windows::core::{HSTRING, PCWSTR};
    // SAFETY: the SID string lives until the call returns; no profile path or computer name.
    match unsafe { windows::Win32::UI::Shell::DeleteProfileW(&HSTRING::from(sid), PCWSTR::null(), PCWSTR::null()) } {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ERROR_FILE_NOT_FOUND.to_hresult() || e.code() == ERROR_PATH_NOT_FOUND.to_hresult() => {
            Ok(())
        }
        Err(e) => Err(e.message()),
    }
}

#[cfg(not(windows))]
fn delete_profile(_sid: &str) -> Result<(), String> {
    Ok(())
}

/// Ends a worker process that didn't stop in time; true when it has ended.
#[cfg(windows)]
fn end_process(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
    };
    // SAFETY: the handle is opened for this process ID, used, and closed.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid) else { return false };
        let ended = TerminateProcess(handle, 1).is_ok() && WaitForSingleObject(handle, 5000) == WAIT_OBJECT_0;
        let _ = CloseHandle(handle);
        if ended {
            log::warn!("Ended w3wp {pid}");
        }
        ended
    }
}

#[cfg(not(windows))]
fn end_process(_pid: u32) -> bool {
    false
}

/// What a failed appcmd said. In XML mode it answers `<ERROR message="…" />`; without administrator rights it ends
/// with code 5 and "insufficient permissions" - say what that means instead.
fn access_message(output: &ProcessOutput) -> String {
    let message = document(&output.stdout)
        .ok()
        .and_then(|doc| {
            doc.descendants().find(|n| n.has_tag_name("ERROR")).and_then(|n| n.attribute("message").map(str::to_string))
        })
        .map(|m| m.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| output.message());
    let lower = message.to_ascii_lowercase();
    if output.exit_code == Some(5)
        || message.contains("80070005")
        || lower.contains("access is denied")
        || lower.contains("insufficient permissions")
    {
        "IIS's configuration can only be read with administrator rights - start DNN Manager as administrator".into()
    } else {
        message
    }
}

/// `sc query W3SVC`: the state's number is the same in every Windows language.
pub fn parse_service_state(output: &str) -> IisServerState {
    if output.contains("1060") {
        return IisServerState::NotInstalled;
    }
    let state = output
        .lines()
        .find(|l| l.trim_start().starts_with("STATE"))
        .and_then(|l| l.split(':').nth(1))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|n| n.parse::<u32>().ok());
    match state {
        Some(1) => IisServerState::Stopped,
        Some(2) => IisServerState::Starting,
        Some(3) => IisServerState::Stopping,
        Some(4) => IisServerState::Running,
        _ => IisServerState::Unknown,
    }
}

fn document(xml: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse(xml).map_err(|e| format!("IIS's answer can't be read: {e}"))
}

/// `appcmd list apppool /xml`: pool name → state.
pub fn parse_app_pools(xml: &str) -> HashMap<String, String> {
    let Ok(doc) = document(xml) else { return HashMap::new() };
    doc.descendants()
        .filter(|n| n.has_tag_name("APPPOOL"))
        .filter_map(|n| Some((n.attribute("APPPOOL.NAME")?.to_string(), n.attribute("state")?.to_string())))
        .collect()
}

/// `appcmd list wp /xml`: pool name → its worker processes' IDs.
pub fn parse_worker_processes(xml: &str) -> HashMap<String, Vec<u32>> {
    let mut map: HashMap<String, Vec<u32>> = HashMap::new();
    let Ok(doc) = document(xml) else { return map };
    for wp in doc.descendants().filter(|n| n.has_tag_name("WP")) {
        if let (Some(pid), Some(pool)) =
            (wp.attribute("WP.NAME").and_then(|p| p.parse().ok()), wp.attribute("APPPOOL.NAME"))
        {
            map.entry(pool.to_string()).or_default().push(pid);
        }
    }
    for pids in map.values_mut() {
        pids.sort_unstable();
    }
    map
}

/// `appcmd list site /config /xml`: every site with its state, bindings, root application's pool and folder.
pub fn parse_sites(
    xml: &str,
    pools: &HashMap<String, String>,
    workers: &HashMap<String, Vec<u32>>,
) -> Result<BTreeMap<String, IisSiteRuntime>, String> {
    let doc = document(xml)?;
    let mut sites = BTreeMap::new();
    for entry in doc.descendants().filter(|n| n.has_tag_name("SITE")) {
        let Some(name) = entry.attribute("SITE.NAME") else { continue };
        let config = entry.children().find(|n| n.has_tag_name("site"));
        let root_app = config.and_then(|c| {
            c.children().filter(|n| n.has_tag_name("application")).find(|a| a.attribute("path").unwrap_or("/") == "/")
        });
        // A site without its own applicationPool runs in the default one.
        let app_pool = root_app
            .and_then(|a| a.attribute("applicationPool"))
            .or_else(|| {
                config
                    .and_then(|c| c.children().find(|n| n.has_tag_name("applicationDefaults")))
                    .and_then(|d| d.attribute("applicationPool"))
            })
            .unwrap_or("DefaultAppPool")
            .to_string();
        let physical_path = root_app
            .and_then(|a| {
                a.children().filter(|n| n.has_tag_name("virtualDirectory")).find(|v| v.attribute("path") == Some("/"))
            })
            .and_then(|v| v.attribute("physicalPath"))
            .map(expand_environment)
            .unwrap_or_default();
        let bindings = config
            .and_then(|c| c.children().find(|n| n.has_tag_name("bindings")))
            .map(|b| {
                b.children()
                    .filter(|n| n.has_tag_name("binding"))
                    .map(|b| {
                        let protocol = b.attribute("protocol").unwrap_or_default();
                        let information = b.attribute("bindingInformation").unwrap_or_default();
                        // The certificate of an https binding is http.sys's, not in this config; IIS 8.5+ notes its hash.
                        let has_certificate = b.attribute("certificateHash").is_some_and(|h| !h.is_empty());
                        IisBinding::parse(protocol, information, has_certificate)
                    })
                    .collect()
            })
            .unwrap_or_default();
        sites.insert(
            name.to_string(),
            IisSiteRuntime {
                id: entry.attribute("SITE.ID").and_then(|i| i.parse().ok()).unwrap_or_default(),
                state: entry.attribute("state").unwrap_or("Unknown").to_string(),
                app_pool_state: pools.get(&app_pool).cloned(),
                worker_process_ids: workers.get(&app_pool).cloned().unwrap_or_default(),
                app_pool,
                bindings,
                physical_path,
            },
        );
    }
    Ok(sites)
}

/// `%SystemDrive%\inetpub\wwwroot` → `C:\inetpub\wwwroot`; an unknown variable stays as it is.
pub fn expand_environment(path: &str) -> PathBuf {
    let mut result = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('%') {
        let Some(len) = rest[start + 1..].find('%') else { break };
        let name = &rest[start + 1..start + 1 + len];
        result.push_str(&rest[..start]);
        match std::env::var(name) {
            Ok(value) if !name.is_empty() => result.push_str(&value),
            _ => result.push_str(&rest[start..start + len + 2]),
        }
        rest = &rest[start + len + 2..];
    }
    result.push_str(rest);
    PathBuf::from(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)] // only the Windows-only tests use it
    const SITES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<appcmd>
    <SITE SITE.NAME="Default Web Site" SITE.ID="1" bindings="http/*:80:" state="Stopped">
        <site name="Default Web Site" id="1">
            <bindings><binding protocol="http" bindingInformation="*:80:" /></bindings>
            <application path="/"><virtualDirectory path="/" physicalPath="%SystemDrive%\inetpub\wwwroot" /></application>
        </site>
    </SITE>
    <SITE SITE.NAME="dnn10" SITE.ID="3" bindings="http/*:80:dnn10.dnndev.me" state="Started">
        <site name="dnn10" id="3">
            <bindings>
                <binding protocol="http" bindingInformation="*:80:dnn10.dnndev.me" />
                <binding protocol="https" bindingInformation="*:443:dnn10.dnndev.me" sslFlags="1" certificateHash="ab12" />
            </bindings>
            <application path="/" applicationPool="dnn10">
                <virtualDirectory path="/" physicalPath="C:\DNN\dnn10" />
            </application>
            <application path="/api" applicationPool="other"><virtualDirectory path="/" physicalPath="C:\x" /></application>
        </site>
    </SITE>
</appcmd>"#;

    #[cfg(windows)] // Windows paths, drives or programs
    #[test]
    fn reads_sites_with_their_pools_and_workers() {
        let pools = parse_app_pools(
            r#"<appcmd><APPPOOL APPPOOL.NAME="dnn10" state="Started" /><APPPOOL APPPOOL.NAME="DefaultAppPool" state="Stopped" /></appcmd>"#,
        );
        let workers = parse_worker_processes(
            r#"<appcmd><WP WP.NAME="5120" APPPOOL.NAME="dnn10" /><WP WP.NAME="812" APPPOOL.NAME="dnn10" /></appcmd>"#,
        );
        let sites = parse_sites(SITES, &pools, &workers).unwrap();
        let dnn = &sites["dnn10"];
        assert_eq!((dnn.id, dnn.state.as_str(), dnn.app_pool.as_str()), (3, "Started", "dnn10"));
        assert_eq!(dnn.app_pool_state.as_deref(), Some("Started"));
        assert_eq!(dnn.worker_process_ids, vec![812, 5120]);
        assert_eq!(dnn.physical_path, PathBuf::from(r"C:\DNN\dnn10"));
        assert_eq!(dnn.browse_url().as_deref(), Some("https://dnn10.dnndev.me"));

        let default = &sites["Default Web Site"];
        assert_eq!(default.app_pool, "DefaultAppPool");
        assert_eq!(default.app_pool_state.as_deref(), Some("Stopped"));
        assert!(!default.physical_path.to_string_lossy().contains("%SystemDrive%"));
    }

    #[test]
    fn reads_the_service_state_by_number() {
        let running = "SERVICE_NAME: W3SVC\r\n        TYPE               : 20  WIN32_SHARE_PROCESS\r\n        STATE              : 4  RUNNING\r\n";
        assert_eq!(parse_service_state(running), IisServerState::Running);
        assert_eq!(parse_service_state("STATE : 1  STOPPED"), IisServerState::Stopped);
        assert_eq!(parse_service_state("[SC] OpenService FAILED 1060:"), IisServerState::NotInstalled);
        assert_eq!(parse_service_state(""), IisServerState::Unknown);
    }

    #[test]
    fn expands_known_variables_only() {
        unsafe { std::env::set_var("DNNM_TEST_DRIVE", "D:") };
        assert_eq!(expand_environment(r"%DNNM_TEST_DRIVE%\sites"), PathBuf::from(r"D:\sites"));
        assert_eq!(expand_environment(r"%NO_SUCH_VAR_X%\a"), PathBuf::from(r"%NO_SUCH_VAR_X%\a"));
        assert_eq!(expand_environment("100%"), PathBuf::from("100%"));
    }

    #[test]
    fn explains_appcmd_errors() {
        // What appcmd answers without administrator rights, word for word.
        let denied = ProcessOutput {
            exit_code: Some(5),
            stdout: "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n<appcmd>\r\n    <ERROR message=\"Configuration error &#xD;&#xA;Filename: redirection.config&#xD;&#xA;Line Number: 0&#xD;&#xA;Description: Cannot read configuration file due to insufficient permissions&#xD;&#xA;.\" />\r\n</appcmd>".into(),
            stderr: String::new(),
        };
        assert!(access_message(&denied).contains("administrator rights"));
        let other = ProcessOutput {
            exit_code: Some(1168),
            stdout: r#"<appcmd><ERROR message="Cannot find SITE object with identifier &quot;x&quot;" /></appcmd>"#
                .into(),
            stderr: String::new(),
        };
        assert_eq!(access_message(&other), "Cannot find SITE object with identifier \"x\"");
    }

    #[test]
    fn bad_xml_is_an_error_not_a_panic() {
        assert!(parse_sites("<appcmd><SITE", &HashMap::new(), &HashMap::new()).is_err());
        assert!(parse_app_pools("not xml").is_empty());
    }
}
