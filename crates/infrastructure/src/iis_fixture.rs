//! A stand-in IIS for working on and checking the UI without administrator rights: the sites come from a file in
//! appcmd's own XML (`appcmd list site /config /xml`), and every change - Start / Stop, create, remove, rename,
//! bindings, app pool settings - changes only this in-memory copy: nothing in IIS, no folder's permissions, no
//! profile, no database. Used when `DNNMANAGER_GPUI_FIXTURE` names such a file.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use dnnm_application::iis::{
    IisBinding, IisBindingDetails, IisError, IisManager, IisPoolDetails, IisPoolSettings, IisServerAction,
    IisServerState, IisSiteDetails, IisSiteRuntime, NO_MANAGED_CODE, pool_account, states,
};
use dnnm_application::project::SiteTraffic;

use crate::iis::{expand_environment, parse_sites};

pub struct FixtureIis {
    sites: Mutex<BTreeMap<String, IisSiteRuntime>>,
    /// App pools whose settings were read or changed - the others have IIS's defaults.
    pools: Mutex<HashMap<String, IisPoolDetails>>,
    server: Mutex<IisServerState>,
}

fn lock<T>(mutex: &Mutex<T>) -> Result<MutexGuard<'_, T>, IisError> {
    mutex.lock().map_err(|_| IisError("fixture lock".into()))
}

/// A pool as IIS makes one: .NET v4.0, Integrated, ApplicationPoolIdentity, IIS's own defaults.
fn default_pool(name: &str) -> IisPoolDetails {
    IisPoolDetails {
        name: name.to_string(),
        state: Some(states::STARTED.into()),
        runtime: "v4.0".into(),
        pipeline: "Integrated".into(),
        identity_type: "ApplicationPoolIdentity".into(),
        account: pool_account(name, "ApplicationPoolIdentity", ""),
        start_mode: "OnDemand".into(),
        idle_timeout: Duration::from_secs(20 * 60),
        idle_timeout_action: Some("Terminate".into()),
        recycle_interval: Duration::from_secs(29 * 3600),
        recycle_at: Vec::new(),
        private_memory_limit_kb: 0,
        virtual_memory_limit_kb: 0,
        queue_length: 1000,
        enable_32bit: false,
        rapid_fail_protection: true,
        rapid_fail_max_crashes: 5,
        rapid_fail_interval: Duration::from_secs(300),
        max_processes: 1,
        load_user_profile: true,
        worker_process_ids: Vec::new(),
    }
}

impl FixtureIis {
    pub fn load(file: &Path) -> Result<Self, String> {
        let xml = std::fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
        let sites = parse_sites(&xml, &Default::default(), &Default::default())?;
        Ok(Self {
            sites: Mutex::new(sites),
            pools: Mutex::new(HashMap::new()),
            server: Mutex::new(IisServerState::Running),
        })
    }

    fn name_of(sites: &BTreeMap<String, IisSiteRuntime>, site: &str) -> Result<String, IisError> {
        sites
            .keys()
            .find(|k| k.eq_ignore_ascii_case(site))
            .cloned()
            .ok_or_else(|| IisError(format!("Site '{site}' not found")))
    }

    fn set_state(&self, site: &str, state: &str) -> Result<(), IisError> {
        let mut sites = lock(&self.sites)?;
        let name = Self::name_of(&sites, site)?;
        let entry = sites.get_mut(&name).expect("found");
        entry.state = state.to_string();
        entry.app_pool_state = Some(state.to_string());
        Ok(())
    }

    fn pool(&self, name: &str) -> Result<IisPoolDetails, IisError> {
        Ok(lock(&self.pools)?.get(name).cloned().unwrap_or_else(|| default_pool(name)))
    }

    fn shared(sites: &BTreeMap<String, IisSiteRuntime>, site: &str, pool: &str) -> bool {
        sites.iter().any(|(name, s)| !name.eq_ignore_ascii_case(site) && s.app_pool.eq_ignore_ascii_case(pool))
    }
}

impl IisManager for FixtureIis {
    fn is_available(&self) -> bool {
        true
    }

    fn server_state(&self) -> IisServerState {
        self.server.lock().map(|s| *s).unwrap_or(IisServerState::Unknown)
    }

    fn site_runtimes(&self) -> Result<BTreeMap<String, IisSiteRuntime>, IisError> {
        Ok(lock(&self.sites)?.clone())
    }

    fn start_site(&self, site: &str) -> Result<(), IisError> {
        self.set_state(site, states::STARTED)
    }

    fn stop_site(&self, site: &str) -> Result<(), IisError> {
        self.set_state(site, states::STOPPED)
    }

    fn restart_site(&self, site: &str) -> Result<(), IisError> {
        self.set_state(site, states::STARTED)
    }

    fn recycle_app_pool(&self, _site: &str) -> Result<(), IisError> {
        Ok(())
    }

    fn control_server(&self, action: IisServerAction) -> Result<(), IisError> {
        let mut server = lock(&self.server)?;
        *server = match action {
            IisServerAction::Stop => IisServerState::Stopped,
            IisServerAction::Start | IisServerAction::Restart => IisServerState::Running,
        };
        Ok(())
    }

    fn create_site(&self, site: &str, physical_path: &Path, host: &str, port: u16) -> Result<(), IisError> {
        self.remove_site(site)?;
        let mut sites = lock(&self.sites)?;
        let id = sites.values().map(|s| s.id).max().unwrap_or(0) + 1;
        let runtime = IisSiteRuntime {
            id,
            state: states::STOPPED.into(),
            app_pool: site.to_string(),
            app_pool_state: Some(states::STOPPED.into()),
            worker_process_ids: Vec::new(),
            bindings: vec![IisBinding::parse("http", &format!("*:{port}:{host}"), false)],
            physical_path: physical_path.to_path_buf(),
        };
        sites.insert(site.to_string(), runtime);
        Ok(())
    }

    fn remove_site(&self, site: &str) -> Result<(), IisError> {
        let mut sites = lock(&self.sites)?;
        if let Ok(name) = Self::name_of(&sites, site) {
            sites.remove(&name);
        }
        if !Self::shared(&sites, site, site) {
            lock(&self.pools)?.remove(site);
        }
        Ok(())
    }

    fn stop_site_and_wait(&self, site: &str, _timeout: Duration) -> Result<(), IisError> {
        self.stop_site(site)
    }

    fn log_directory(&self, site: &str) -> Option<PathBuf> {
        let sites = self.sites.lock().ok()?;
        let id = sites.iter().find(|(name, _)| name.eq_ignore_ascii_case(site))?.1.id;
        Some(expand_environment(r"%SystemDrive%\inetpub\logs\LogFiles").join(format!("W3SVC{id}")))
    }

    fn app_pool_identity(&self, site: &str) -> String {
        let pool = self
            .site_runtimes()
            .ok()
            .and_then(|s| s.iter().find(|(n, _)| n.eq_ignore_ascii_case(site)).map(|(_, r)| r.app_pool.clone()))
            .unwrap_or_else(|| site.to_string());
        self.pool(&pool).map_or_else(|_| format!(r"IIS APPPOOL\{pool}"), |p| p.account)
    }

    fn enable_user_profile(&self, site: &str) -> Result<(), IisError> {
        let pool = {
            let sites = lock(&self.sites)?;
            let name =
                Self::name_of(&sites, site).map_err(|_| IisError(format!("IIS site '{site}' has no app pool.")))?;
            sites[&name].app_pool.clone()
        };
        let mut details = self.pool(&pool)?;
        details.load_user_profile = true;
        lock(&self.pools)?.insert(pool, details);
        Ok(())
    }

    fn site_traffic(&self) -> HashMap<String, SiteTraffic> {
        HashMap::new()
    }

    fn requests_served(&self) -> HashMap<String, u64> {
        HashMap::new()
    }

    fn site_details(&self, site: &str) -> Option<IisSiteDetails> {
        let sites = self.sites.lock().ok()?;
        let (name, runtime) = sites.iter().find(|(name, _)| name.eq_ignore_ascii_case(site))?;
        let mut pool = self.pool(&runtime.app_pool).ok()?;
        pool.state = runtime.app_pool_state.clone();
        pool.worker_process_ids = runtime.worker_process_ids.clone();
        let bindings = runtime
            .bindings
            .iter()
            .map(|b| {
                let port = b.port.map(|p| p.to_string()).unwrap_or_default();
                IisBindingDetails::parse(&b.protocol, &format!("{}:{port}:{}", b.address, b.host), false)
            })
            .collect();
        Some(IisSiteDetails {
            id: runtime.id,
            name: name.clone(),
            state: runtime.state.clone(),
            physical_path: runtime.physical_path.clone(),
            config_path: format!("MACHINE/WEBROOT/APPHOST/{name}"),
            server_auto_start: true,
            preload_enabled: Some(false),
            bindings,
            pool: Some(pool),
        })
    }

    fn replace_http_bindings(&self, site: &str, bindings: &[(String, u16)]) -> Result<(), IisError> {
        let mut sites = lock(&self.sites)?;
        let name = Self::name_of(&sites, site)?;
        let entry = sites.get_mut(&name).expect("found");
        let mut addresses = HashMap::new();
        for old in entry.bindings.iter().filter(|b| b.protocol.eq_ignore_ascii_case("http")) {
            if let Some(port) = old.port {
                addresses.entry((old.host.to_lowercase(), port)).or_insert_with(|| old.address.clone());
            }
        }
        entry.bindings.retain(|b| !b.protocol.eq_ignore_ascii_case("http"));
        for (host, port) in bindings {
            let address = addresses.get(&(host.to_lowercase(), *port)).cloned().unwrap_or_else(|| "*".into());
            entry.bindings.push(IisBinding::parse("http", &format!("{address}:{port}:{host}"), false));
        }
        Ok(())
    }

    fn set_pool_settings(&self, site: &str, settings: &IisPoolSettings) -> Result<(), IisError> {
        let pool = {
            let sites = lock(&self.sites)?;
            let name =
                Self::name_of(&sites, site).map_err(|_| IisError(format!("IIS site '{site}' has no app pool.")))?;
            let pool = sites[&name].app_pool.clone();
            if Self::shared(&sites, &name, &pool) {
                return Err(IisError(format!(
                    "The app pool '{pool}' is used by other sites too - change it in IIS Manager."
                )));
            }
            pool
        };
        let mut details = self.pool(&pool)?;
        details.runtime = if settings.runtime.is_empty() { NO_MANAGED_CODE.into() } else { settings.runtime.clone() };
        details.pipeline = settings.pipeline.clone();
        details.enable_32bit = settings.enable_32bit;
        if settings.identity != "SpecificUser" {
            details.identity_type = settings.identity.clone();
            details.account = pool_account(&pool, &settings.identity, "");
        }
        details.idle_timeout = settings.idle_timeout;
        details.start_mode = settings.start_mode.clone();
        lock(&self.pools)?.insert(pool, details);
        Ok(())
    }

    fn rename_site(&self, site: &str, new_name: &str, physical_path: &Path) -> Result<(), IisError> {
        let mut sites = lock(&self.sites)?;
        let name = Self::name_of(&sites, site)?;
        if !new_name.eq_ignore_ascii_case(site) && Self::name_of(&sites, new_name).is_ok() {
            return Err(IisError(format!("IIS already has a site named '{new_name}'.")));
        }
        let mut entry = sites.remove(&name).expect("found");
        let rename_pool = entry.app_pool.eq_ignore_ascii_case(site) && !Self::shared(&sites, site, &entry.app_pool);
        if rename_pool {
            let mut pools = lock(&self.pools)?;
            if let Some(mut details) = pools.remove(&entry.app_pool) {
                details.name = new_name.to_string();
                pools.insert(new_name.to_string(), details);
            }
            entry.app_pool = new_name.to_string();
        }
        entry.state = states::STOPPED.into();
        entry.app_pool_state = Some(states::STOPPED.into());
        entry.physical_path = physical_path.to_path_buf();
        sites.insert(new_name.to_string(), entry);
        Ok(())
    }

    fn grant_permissions(&self, _path: &Path, _identities: &[String]) -> Result<(), IisError> {
        Ok(())
    }

    fn remove_app_pool_profile(&self, _pool: &str) -> Result<(), IisError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, FixtureIis) {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sites.xml");
        std::fs::write(
            &file,
            r#"<appcmd><SITE SITE.NAME="a" SITE.ID="1" state="Started"><site name="a" id="1">
               <bindings><binding protocol="http" bindingInformation="127.0.0.2:80:a.me" />
                         <binding protocol="https" bindingInformation="*:443:a.me" /></bindings>
               <application path="/" applicationPool="a"><virtualDirectory path="/" physicalPath="C:\a" /></application>
               </site></SITE></appcmd>"#,
        )
        .unwrap();
        let iis = FixtureIis::load(&file).unwrap();
        (dir, iis)
    }

    #[test]
    fn start_and_stop_change_only_the_copy() {
        let (_dir, iis) = fixture();
        iis.stop_site("a").unwrap();
        assert_eq!(iis.site_runtimes().unwrap()["a"].state, "Stopped");
        assert!(iis.start_site("missing").is_err());
        iis.control_server(IisServerAction::Stop).unwrap();
        assert_eq!(iis.server_state(), IisServerState::Stopped);
    }

    #[test]
    fn creates_renames_and_removes_in_memory() {
        let (_dir, iis) = fixture();
        iis.create_site("b", Path::new(r"C:\b"), "b.me", 8080).unwrap();
        let b = &iis.site_runtimes().unwrap()["b"];
        assert_eq!((b.id, b.browse_url().as_deref()), (2, Some("http://b.me:8080")));
        iis.rename_site("b", "c", Path::new(r"C:\c")).unwrap();
        let sites = iis.site_runtimes().unwrap();
        assert_eq!((sites["c"].app_pool.as_str(), sites["c"].physical_path.as_path()), ("c", Path::new(r"C:\c")));
        assert!(iis.rename_site("c", "A", Path::new(r"C:\c")).unwrap_err().0.contains("already has a site"));
        iis.remove_site("c").unwrap();
        assert!(!iis.site_runtimes().unwrap().contains_key("c"));
        assert!(iis.log_directory("a").unwrap().ends_with("W3SVC1"));
    }

    #[test]
    fn bindings_and_pool_settings_in_memory() {
        let (_dir, iis) = fixture();
        iis.replace_http_bindings("a", &[("A.me".into(), 80), ("".into(), 81)]).unwrap();
        let bindings = &iis.site_runtimes().unwrap()["a"].bindings;
        let info: Vec<_> =
            bindings.iter().map(|b| format!("{}:{}:{:?}:{}", b.protocol, b.address, b.port, b.host)).collect();
        assert_eq!(info, ["https:*:Some(443):a.me", "http:127.0.0.2:Some(80):A.me", "http:*:Some(81):"]);

        let mut settings = IisPoolSettings::from_pool(iis.site_details("a").unwrap().pool.as_ref().unwrap());
        settings.identity = "NetworkService".into();
        settings.runtime = String::new();
        iis.set_pool_settings("a", &settings).unwrap();
        let pool = iis.site_details("a").unwrap().pool.unwrap();
        assert_eq!((pool.runtime.as_str(), pool.account.as_str()), (NO_MANAGED_CODE, r"NT AUTHORITY\NETWORK SERVICE"));
        assert_eq!(iis.app_pool_identity("a"), r"NT AUTHORITY\NETWORK SERVICE");
    }
}
