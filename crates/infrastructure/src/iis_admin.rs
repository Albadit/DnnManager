//! IIS administration through appcmd (`IisManager.cs`'s writes and its detailed reads): the argument lists DNN
//! Manager passes to appcmd and icacls, and the parsers of the configuration it reads. Kept apart from running them so
//! every command can be checked without changing IIS.
//!
//! Why appcmd and not IIS's COM API (AHADMIN, `IAppHostWritableAdminManager`), operation by operation - appcmd
//! commits each command as one change to applicationHost.config, as `ServerManager.CommitChanges` does:
//!
//! | Operation | How |
//! |---|---|
//! | Create a site and its pool | `add apppool`, `add site`, `set site /applicationDefaults.applicationPool` |
//! | Remove, stop, start | `stop`/`start`/`delete site`, `stop`/`start`/`delete apppool` |
//! | Rename | `set site /name`, `set apppool /name`, `set app /applicationPool`, `set vdir /physicalPath` |
//! | Bindings | one `set site` with `/-bindings.[…]` and `/+bindings.[…]` |
//! | Pool settings, user profile | `set apppool` with the attributes |
//! | Details, log folder, identity | `list config /section:…/sites` and `…/applicationPools` (applicationHost.config as it is, the defaults elements included), state from `list site`, `list apppool`, `list wp` |
//!
//! None of it needs COM: appcmd can set every attribute the C# sets. COM would only save starting a process per
//! command - not worth the interop for operations a user starts one at a time.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use dnnm_application::iis::{IisBindingDetails, IisPoolDetails, IisPoolSettings, NO_MANAGED_CODE, pool_account};

use crate::iis::expand_environment;
use crate::sha1::sha1;

/// App pools Windows ships with - their shared profiles are never deleted, even for a project (pathologically) named
/// the same.
pub const RESERVED_APP_POOLS: [&str; 6] =
    ["DefaultAppPool", "Classic .NET AppPool", ".NET v2.0", ".NET v2.0 Classic", ".NET v4.5", ".NET v4.5 Classic"];

/// Where IIS writes request logs when neither the site nor the site defaults say.
const DEFAULT_LOG_DIRECTORY: &str = r"%SystemDrive%\inetpub\logs\LogFiles";

// ─── Argument lists ──────────────────────────────────────────────────────

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

/// The new site's own pool, as the C# makes it: .NET v4.0, Integrated.
pub fn add_pool_args(pool: &str) -> Vec<String> {
    args(&[
        "add",
        "apppool",
        &format!("/name:{pool}"),
        "/managedRuntimeVersion:v4.0",
        "/managedPipelineMode:Integrated",
    ])
}

/// The site, bound to http `host` on `port` on every address, serving `physical_path`.
pub fn add_site_args(site: &str, physical_path: &str, host: &str, port: u16) -> Vec<String> {
    args(&[
        "add",
        "site",
        &format!("/name:{site}"),
        &format!("/bindings:http/*:{port}:{host}"),
        &format!("/physicalPath:{physical_path}"),
    ])
}

/// Its applications run in `pool` (`site.ApplicationDefaults.ApplicationPoolName`).
pub fn site_pool_args(site: &str, pool: &str) -> Vec<String> {
    args(&["set", "site", &format!("/site.name:{site}"), &format!("/applicationDefaults.applicationPool:{pool}")])
}

/// `start`, `stop`, `recycle` or `delete` of a site (`kind` "site") or an app pool (`kind` "apppool").
pub fn object_args(verb: &str, kind: &str, name: &str) -> Vec<String> {
    let key = if kind == "site" { "site.name" } else { "apppool.name" };
    args(&[verb, kind, &format!("/{key}:{name}")])
}

/// One `set site` that removes the site's http bindings and adds `bindings` (host, port) - each new one on the
/// address it had (127.0.0.2:80:shop stays on 127.0.0.2), a new one on every address. Its other bindings stay.
pub fn replace_bindings_args(site: &str, current: &[IisBindingDetails], bindings: &[(String, u16)]) -> Vec<String> {
    let mut addresses: HashMap<(String, u16), String> = HashMap::new();
    let mut out = args(&["set", "site", &format!("/site.name:{site}")]);
    for old in current.iter().filter(|b| b.protocol.eq_ignore_ascii_case("http")) {
        if let Some(port) = old.port {
            addresses.entry((old.host.to_lowercase(), port)).or_insert_with(|| old.address.clone());
        }
        out.push(format!("/-bindings.[protocol='{}',bindingInformation='{}']", old.protocol, old.binding_information));
    }
    for (host, port) in bindings {
        let address = addresses.get(&(host.to_lowercase(), *port)).map_or("*", String::as_str);
        out.push(format!("/+bindings.[protocol='http',bindingInformation='{address}:{port}:{host}']"));
    }
    out
}

/// `set apppool` with what Edit app pool changes. A specific account keeps its user and password - they're changed in
/// IIS Manager. The error names a value IIS doesn't know.
pub fn pool_settings_args(pool: &str, settings: &IisPoolSettings) -> Result<Vec<String>, String> {
    let pipeline = ["Integrated", "Classic"]
        .into_iter()
        .find(|p| p.eq_ignore_ascii_case(&settings.pipeline))
        .ok_or_else(|| format!("'{}' isn't a pipeline mode - Integrated or Classic.", settings.pipeline))?;
    let mut out = args(&[
        "set",
        "apppool",
        &format!("/apppool.name:{pool}"),
        &format!("/managedRuntimeVersion:{}", settings.runtime),
        &format!("/managedPipelineMode:{pipeline}"),
        &format!("/enable32BitAppOnWin64:{}", settings.enable_32bit),
    ]);
    if let Some(identity) = ["ApplicationPoolIdentity", "NetworkService", "LocalService", "LocalSystem"]
        .into_iter()
        .find(|i| i.eq_ignore_ascii_case(&settings.identity))
    {
        out.push(format!("/processModel.identityType:{identity}"));
    }
    out.push(format!("/processModel.idleTimeout:{}", format_timespan(settings.idle_timeout)));
    let start = if settings.start_mode.eq_ignore_ascii_case("AlwaysRunning") { "AlwaysRunning" } else { "OnDemand" };
    out.push(format!("/startMode:{start}"));
    Ok(out)
}

/// Loads the identity's profile and points its environment (LOCALAPPDATA…) at it - where LocalDB looks for its
/// instance.
pub fn user_profile_args(pool: &str) -> Vec<String> {
    args(&[
        "set",
        "apppool",
        &format!("/apppool.name:{pool}"),
        "/processModel.loadUserProfile:true",
        "/processModel.setProfileEnvironment:true",
    ])
}

/// The renames of `RenameSite`: the site, its pool when `rename_pool`, each application's pool then, and the root's
/// folder - in that order (the later ones name the site by its new name).
pub fn rename_args(site: &SiteEntry, new_name: &str, rename_pool: bool, physical_path: &str) -> Vec<Vec<String>> {
    let mut commands = vec![args(&["set", "site", &format!("/site.name:{}", site.name), &format!("/name:{new_name}")])];
    if rename_pool {
        commands.push(args(&["set", "apppool", &format!("/apppool.name:{}", site.name), &format!("/name:{new_name}")]));
        for (path, _) in &site.apps {
            commands.push(args(&[
                "set",
                "app",
                &format!("/app.name:{new_name}{path}"),
                &format!("/applicationPool:{new_name}"),
            ]));
        }
    }
    commands.push(args(&[
        "set",
        "vdir",
        &format!("/vdir.name:{new_name}/"),
        &format!("/physicalPath:{physical_path}"),
    ]));
    commands
}

/// The SID IIS gives an app pool's identity (`IIS APPPOOL\<pool>`): S-1-5-82 and five little-endian numbers of the
/// SHA-1 of its lower-cased name in UTF-16. Computed, because a pool made a moment ago often can't be looked up yet.
pub fn app_pool_sid(pool: &str) -> String {
    let utf16: Vec<u8> = pool.to_lowercase().encode_utf16().flat_map(u16::to_le_bytes).collect();
    let hash = sha1(&utf16);
    let mut sid = String::from("S-1-5-82");
    for chunk in hash.as_chunks::<4>().0 {
        sid.push_str(&format!("-{}", u32::from_le_bytes(*chunk)));
    }
    sid
}

/// How icacls names an identity: by SID where it is known (the well-known groups, an app pool), else by name.
fn trustee(identity: &str) -> String {
    const PREFIX: &str = r"IIS APPPOOL\";
    if identity.eq_ignore_ascii_case("IIS_IUSRS") {
        "*S-1-5-32-568".into()
    } else if identity.eq_ignore_ascii_case("IUSR") {
        "*S-1-5-17".into()
    } else if identity.len() > PREFIX.len() && identity[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
        format!("*{}", app_pool_sid(&identity[PREFIX.len()..]))
    } else {
        identity.to_string()
    }
}

/// The two icacls runs that give `identity` its rights to `path` in place of what it had (an added entry only adds to
/// an earlier one - the Full control older versions gave would stay): every entry of it removed, then one granted,
/// inherited by every folder and file. IIS's shared groups (IIS_IUSRS holds every site's app pool) may only read and
/// run; the site's own pool may change it - DNN writes Portals, App_Data, web.config.
pub fn icacls_args(path: &str, identity: &str) -> [Vec<String>; 2] {
    let shared = identity.eq_ignore_ascii_case("IIS_IUSRS") || identity.eq_ignore_ascii_case("IUSR");
    let who = trustee(identity);
    let rights = if shared { "(OI)(CI)RX" } else { "(OI)(CI)M" };
    [args(&[path, "/remove", &who]), args(&[path, "/grant", &format!("{who}:{rights}")])]
}

// ─── Reading the configuration ───────────────────────────────────────────

/// Parses what `appcmd list config /section:…` prints - the section as applicationHost.config has it.
pub fn section_document(text: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse(text).map_err(|e| format!("IIS's answer can't be read: {e}"))
}

/// One site of `appcmd list site /config /xml`: its applications and the pool each runs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteEntry {
    pub name: String,
    pub id: u64,
    pub state: String,
    /// Each application's path ("/", "/api") and pool.
    pub apps: Vec<(String, String)>,
}

impl SiteEntry {
    /// The pool of its root application (`PoolOf`).
    pub fn root_pool(&self) -> Option<&str> {
        self.apps.iter().find(|(path, _)| path == "/").map(|(_, pool)| pool.as_str())
    }
}

/// The sites of `appcmd list site /config /xml` with their applications' pools.
pub fn parse_site_entries(xml: &str) -> Result<Vec<SiteEntry>, String> {
    let doc = section_document(xml)?;
    let mut sites = Vec::new();
    for entry in doc.descendants().filter(|n| n.has_tag_name("SITE")) {
        let Some(name) = entry.attribute("SITE.NAME") else { continue };
        let config = entry.children().find(|n| n.has_tag_name("site"));
        let default_pool = config
            .and_then(|c| c.children().find(|n| n.has_tag_name("applicationDefaults")))
            .and_then(|d| d.attribute("applicationPool"))
            .filter(|p| !p.is_empty())
            .unwrap_or("DefaultAppPool");
        let apps = config
            .map(|c| {
                c.children()
                    .filter(|n| n.has_tag_name("application"))
                    .map(|a| {
                        let pool = a.attribute("applicationPool").filter(|p| !p.is_empty()).unwrap_or(default_pool);
                        (a.attribute("path").unwrap_or("/").to_string(), pool.to_string())
                    })
                    .collect()
            })
            .unwrap_or_default();
        sites.push(SiteEntry {
            name: name.to_string(),
            id: entry.attribute("SITE.ID").and_then(|i| i.parse().ok()).unwrap_or_default(),
            state: entry.attribute("state").unwrap_or("Unknown").to_string(),
            apps,
        });
    }
    Ok(sites)
}

/// Whether an application of another site than `site` - its root or any other - runs in `pool`: stopping or removing
/// that pool would take it down too.
pub fn pool_used_by_others(sites: &[SiteEntry], site: &str, pool: &str) -> bool {
    sites
        .iter()
        .filter(|s| !s.name.eq_ignore_ascii_case(site))
        .flat_map(|s| &s.apps)
        .any(|(_, p)| p.eq_ignore_ascii_case(pool))
}

/// IIS's time spans: `hh:mm:ss` or `d.hh:mm:ss`, seconds possibly with a fraction.
pub fn parse_timespan(text: &str) -> Option<Duration> {
    let (days, rest) = match text.trim().split_once('.') {
        Some((d, rest)) if rest.contains(':') && !d.contains(':') => (d.parse::<u64>().ok()?, rest),
        _ => (0, text.trim()),
    };
    let mut parts = rest.split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(Duration::from_secs(days * 86_400 + h * 3600 + m * 60) + Duration::from_secs_f64(s))
}

/// A time span as IIS writes it: `hh:mm:ss`, with `d.` in front from a day.
pub fn format_timespan(span: Duration) -> String {
    let total = span.as_secs();
    let (days, h, m, s) = (total / 86_400, total / 3600 % 24, total / 60 % 60, total % 60);
    if days > 0 { format!("{days}.{h:02}:{m:02}:{s:02}") } else { format!("{h:02}:{m:02}:{s:02}") }
}

/// An attribute of `element`'s child `child` (or of `element` itself for ""), else the same in the defaults element,
/// else the schema's default - how IIS resolves a pool's or a site's settings.
fn setting<'a>(
    element: roxmltree::Node<'a, 'a>,
    defaults: Option<roxmltree::Node<'a, 'a>>,
    child: &str,
    name: &str,
) -> Option<&'a str> {
    let read = |node: roxmltree::Node<'a, 'a>| {
        let target = if child.is_empty() { Some(node) } else { node.children().find(|c| c.has_tag_name(child)) };
        target.and_then(|t| t.attribute(name))
    };
    read(element).or_else(|| defaults.and_then(read))
}

fn bool_setting(value: Option<&str>, default: bool) -> bool {
    value.map_or(default, |v| v.eq_ignore_ascii_case("true"))
}

fn number_setting(value: Option<&str>, default: u64) -> u64 {
    value.and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

fn span_setting(value: Option<&str>, default: Duration) -> Duration {
    value.and_then(parse_timespan).unwrap_or(default)
}

/// The site element of `name` in the sites section, and the section's `siteDefaults`.
fn find_site<'a>(
    doc: &'a roxmltree::Document<'a>,
    name: &str,
) -> Option<(roxmltree::Node<'a, 'a>, Option<roxmltree::Node<'a, 'a>>)> {
    let site = doc
        .descendants()
        .find(|n| n.has_tag_name("site") && n.attribute("name").is_some_and(|s| s.eq_ignore_ascii_case(name)))?;
    let defaults = doc.descendants().find(|n| n.has_tag_name("siteDefaults"));
    Some((site, defaults))
}

/// The folder IIS writes `site`'s request logs to: its own setting, else the site defaults', else
/// `%SystemDrive%\inetpub\logs\LogFiles` - and in it a folder per site, by its ID (`GetLogDirectory`).
pub fn log_directory_from(sites_section: &str, site: &str) -> Option<PathBuf> {
    let doc = section_document(sites_section).ok()?;
    let (element, defaults) = find_site(&doc, site)?;
    let id = element.attribute("id")?;
    let directory = setting(element, None, "logFile", "directory")
        .filter(|d| !d.is_empty())
        .or_else(|| defaults.and_then(|d| setting(d, None, "logFile", "directory")).filter(|d| !d.is_empty()))
        .unwrap_or(DEFAULT_LOG_DIRECTORY);
    Some(expand_environment(directory).join(format!("W3SVC{id}")))
}

/// What the sites section says of a site for its Details (state and certificates come from elsewhere).
#[derive(Debug, Clone, PartialEq)]
pub struct SiteConfig {
    pub name: String,
    pub id: u64,
    pub physical_path: PathBuf,
    pub server_auto_start: bool,
    pub preload_enabled: Option<bool>,
    pub root_pool: Option<String>,
    pub bindings: Vec<IisBindingDetails>,
}

/// `site` in the sites section of applicationHost.config. The certificate an https binding names (IIS 8.5 and later
/// note it in the binding) is kept as its hash and store; finding it is the caller's.
pub fn site_config_from(sites_section: &str, site: &str) -> Option<SiteConfig> {
    let doc = section_document(sites_section).ok()?;
    let (element, _) = find_site(&doc, site)?;
    let app_defaults = element.children().find(|c| c.has_tag_name("applicationDefaults")).or_else(|| {
        doc.descendants()
            .find(|n| n.has_tag_name("applicationDefaults") && n.parent().is_some_and(|p| p.has_tag_name("sites")))
    });
    let root = element.children().filter(|c| c.has_tag_name("application")).find(|a| a.attribute("path") == Some("/"));
    let physical_path = root
        .and_then(|a| {
            a.children().filter(|v| v.has_tag_name("virtualDirectory")).find(|v| v.attribute("path") == Some("/"))
        })
        .and_then(|v| v.attribute("physicalPath"))
        .map(expand_environment)
        .unwrap_or_default();
    // preloadEnabled is IIS 8's: every IIS DNN Manager runs on has it - unset is its default, off.
    let preload_enabled = root.map(|a| bool_setting(a.attribute("preloadEnabled"), false));
    let root_pool = root.map(|a| {
        a.attribute("applicationPool")
            .or_else(|| app_defaults.and_then(|d| d.attribute("applicationPool")))
            .filter(|p| !p.is_empty())
            .unwrap_or("DefaultAppPool")
            .to_string()
    });
    let bindings = element
        .children()
        .find(|c| c.has_tag_name("bindings"))
        .map(|list| {
            list.children()
                .filter(|b| b.has_tag_name("binding"))
                .map(|b| {
                    let protocol = b.attribute("protocol").unwrap_or_default();
                    let sni = number_setting(b.attribute("sslFlags"), 0) & 1 == 1;
                    let mut details =
                        IisBindingDetails::parse(protocol, b.attribute("bindingInformation").unwrap_or_default(), sni);
                    if let Some(hash) = b.attribute("certificateHash").filter(|h| !h.is_empty()) {
                        details.certificate_hash = Some(hash.to_ascii_uppercase());
                        details.certificate_store =
                            Some(b.attribute("certificateStoreName").filter(|s| !s.is_empty()).unwrap_or("My").into());
                    }
                    details
                })
                .collect()
        })
        .unwrap_or_default();
    Some(SiteConfig {
        name: element.attribute("name").unwrap_or(site).to_string(),
        id: element.attribute("id").and_then(|i| i.parse().ok()).unwrap_or_default(),
        physical_path,
        server_auto_start: bool_setting(element.attribute("serverAutoStart"), true),
        preload_enabled,
        root_pool,
        bindings,
    })
}

/// `pool` in the applicationPools section, its unset settings from `applicationPoolDefaults`, then IIS's schema
/// defaults - its state and worker processes are the caller's to add. None when there is no such pool.
pub fn pool_details_from(pools_section: &str, pool: &str) -> Option<IisPoolDetails> {
    let doc = section_document(pools_section).ok()?;
    let element = doc
        .descendants()
        .find(|n| n.has_tag_name("add") && n.attribute("name").is_some_and(|p| p.eq_ignore_ascii_case(pool)))?;
    let defaults = doc.descendants().find(|n| n.has_tag_name("applicationPoolDefaults"));
    let get = |child: &str, name: &str| setting(element, defaults, child, name);
    let name = element.attribute("name").unwrap_or(pool).to_string();

    let runtime = get("", "managedRuntimeVersion").unwrap_or("v4.0");
    let identity_type = get("processModel", "identityType").unwrap_or("ApplicationPoolIdentity").to_string();
    let account = pool_account(&name, &identity_type, get("processModel", "userName").unwrap_or_default());
    fn restart<'a>(node: roxmltree::Node<'a, 'a>) -> Option<roxmltree::Node<'a, 'a>> {
        node.children()
            .find(|c| c.has_tag_name("recycling"))
            .and_then(|r| r.children().find(|c| c.has_tag_name("periodicRestart")))
    }
    let periodic = |attribute: &str| {
        restart(element)
            .and_then(|p| p.attribute(attribute))
            .or_else(|| defaults.and_then(restart).and_then(|p| p.attribute(attribute)))
    };
    let recycle_at = element
        .descendants()
        .find(|c| c.has_tag_name("schedule"))
        .map(|s| {
            s.children()
                .filter(|c| c.has_tag_name("add"))
                .filter_map(|c| c.attribute("value").and_then(parse_timespan))
                .collect()
        })
        .unwrap_or_default();
    Some(IisPoolDetails {
        state: None,
        runtime: if runtime.is_empty() { NO_MANAGED_CODE.into() } else { runtime.into() },
        pipeline: get("", "managedPipelineMode").unwrap_or("Integrated").to_string(),
        account,
        identity_type,
        start_mode: get("", "startMode").unwrap_or("OnDemand").to_string(),
        idle_timeout: span_setting(get("processModel", "idleTimeout"), Duration::from_secs(20 * 60)),
        idle_timeout_action: Some(get("processModel", "idleTimeoutAction").unwrap_or("Terminate").to_string()),
        recycle_interval: span_setting(periodic("time"), Duration::from_secs(29 * 3600)),
        recycle_at,
        private_memory_limit_kb: number_setting(periodic("privateMemory"), 0),
        virtual_memory_limit_kb: number_setting(periodic("memory"), 0),
        queue_length: number_setting(get("", "queueLength"), 1000),
        enable_32bit: bool_setting(get("", "enable32BitAppOnWin64"), false),
        rapid_fail_protection: bool_setting(get("failure", "rapidFailProtection"), true),
        rapid_fail_max_crashes: number_setting(get("failure", "rapidFailProtectionMaxCrashes"), 5),
        rapid_fail_interval: span_setting(get("failure", "rapidFailProtectionInterval"), Duration::from_secs(300)),
        max_processes: number_setting(get("processModel", "maxProcesses"), 1),
        load_user_profile: bool_setting(get("processModel", "loadUserProfile"), false),
        worker_process_ids: Vec::new(),
        name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sites section as `appcmd list config /section:system.applicationHost/sites` prints it - built from
    /// applicationHost.config's documented schema (reading it needs administrator rights).
    const SITES: &str = r#"<system.applicationHost>
    <sites>
        <site name="Default Web Site" id="1" serverAutoStart="true">
            <application path="/">
                <virtualDirectory path="/" physicalPath="%SystemDrive%\inetpub\wwwroot" />
            </application>
            <bindings><binding protocol="http" bindingInformation="*:80:" /></bindings>
            <logFile directory="D:\logs" />
        </site>
        <site name="shop" id="4" serverAutoStart="false">
            <application path="/" applicationPool="shop" preloadEnabled="true">
                <virtualDirectory path="/" physicalPath="C:\DNN\shop" />
            </application>
            <application path="/api" applicationPool="shared"><virtualDirectory path="/" physicalPath="C:\x" /></application>
            <bindings>
                <binding protocol="http" bindingInformation="127.0.0.2:80:shop.dnndev.me" />
                <binding protocol="https" bindingInformation="*:443:shop.dnndev.me" sslFlags="1" certificateHash="ab12cd" certificateStoreName="WebHosting" />
                <binding protocol="https" bindingInformation="*:8443:" sslFlags="0" />
            </bindings>
        </site>
        <siteDefaults>
            <logFile logFormat="W3C" directory="E:\iis" />
        </siteDefaults>
        <applicationDefaults applicationPool="DefaultAppPool" />
    </sites>
</system.applicationHost>"#;

    const POOLS: &str = r#"<system.applicationHost>
    <applicationPools>
        <add name="DefaultAppPool" />
        <add name="shop" managedRuntimeVersion="" startMode="AlwaysRunning" enable32BitAppOnWin64="true">
            <processModel identityType="SpecificUser" userName="PC\dev" password="[enc:IISCngProvider:x:enc]" idleTimeout="00:00:00" maxProcesses="2" />
            <recycling>
                <periodicRestart time="00:00:00" privateMemory="1048576">
                    <schedule><add value="03:00:00" /><add value="15:30:00" /></schedule>
                </periodicRestart>
            </recycling>
            <failure rapidFailProtection="false" />
        </add>
        <applicationPoolDefaults managedRuntimeVersion="v4.0">
            <processModel identityType="ApplicationPoolIdentity" loadUserProfile="true" setProfileEnvironment="false" />
        </applicationPoolDefaults>
    </applicationPools>
</system.applicationHost>"#;

    #[test]
    fn builds_the_create_commands() {
        assert_eq!(
            add_pool_args("shop"),
            ["add", "apppool", "/name:shop", "/managedRuntimeVersion:v4.0", "/managedPipelineMode:Integrated"]
        );
        assert_eq!(
            add_site_args("shop", r"C:\DNN Sites\shop", "shop.dnndev.me", 8080),
            ["add", "site", "/name:shop", "/bindings:http/*:8080:shop.dnndev.me", r"/physicalPath:C:\DNN Sites\shop"]
        );
        assert_eq!(
            site_pool_args("shop", "shop"),
            ["set", "site", "/site.name:shop", "/applicationDefaults.applicationPool:shop"]
        );
        assert_eq!(object_args("delete", "apppool", "my pool"), ["delete", "apppool", "/apppool.name:my pool"]);
        assert_eq!(object_args("stop", "site", "shop"), ["stop", "site", "/site.name:shop"]);
    }

    #[test]
    fn replacing_bindings_keeps_https_and_each_kept_bindings_address() {
        let current = site_config_from(SITES, "shop").unwrap().bindings;
        let new = [("SHOP.dnndev.me".to_string(), 80), (String::new(), 8080)];
        assert_eq!(
            replace_bindings_args("shop", &current, &new),
            [
                "set",
                "site",
                "/site.name:shop",
                "/-bindings.[protocol='http',bindingInformation='127.0.0.2:80:shop.dnndev.me']",
                "/+bindings.[protocol='http',bindingInformation='127.0.0.2:80:SHOP.dnndev.me']",
                "/+bindings.[protocol='http',bindingInformation='*:8080:']",
            ]
        );
    }

    #[test]
    fn pool_settings_keep_a_specific_user_and_refuse_an_unknown_pipeline() {
        let mut settings = IisPoolSettings {
            runtime: String::new(),
            pipeline: "classic".into(),
            enable_32bit: false,
            identity: "networkservice".into(),
            idle_timeout: Duration::from_secs(90 * 60),
            start_mode: "alwaysrunning".into(),
        };
        assert_eq!(
            pool_settings_args("shop", &settings).unwrap(),
            [
                "set",
                "apppool",
                "/apppool.name:shop",
                "/managedRuntimeVersion:",
                "/managedPipelineMode:Classic",
                "/enable32BitAppOnWin64:false",
                "/processModel.identityType:NetworkService",
                "/processModel.idleTimeout:01:30:00",
                "/startMode:AlwaysRunning",
            ]
        );
        settings.identity = "SpecificUser".into();
        assert!(!pool_settings_args("shop", &settings).unwrap().iter().any(|a| a.contains("identityType")));
        settings.pipeline = "Fast".into();
        assert!(pool_settings_args("shop", &settings).unwrap_err().contains("'Fast'"));
        assert_eq!(
            user_profile_args("shop")[3..],
            ["/processModel.loadUserProfile:true", "/processModel.setProfileEnvironment:true"]
        );
    }

    #[test]
    fn renames_the_site_its_pool_its_applications_and_its_folder() {
        let site = SiteEntry {
            name: "shop".into(),
            id: 4,
            state: "Stopped".into(),
            apps: vec![("/".into(), "shop".into()), ("/api".into(), "shop".into())],
        };
        let commands = rename_args(&site, "store", true, r"C:\DNN\store");
        assert_eq!(commands[0], ["set", "site", "/site.name:shop", "/name:store"]);
        assert_eq!(commands[1], ["set", "apppool", "/apppool.name:shop", "/name:store"]);
        assert_eq!(commands[2], ["set", "app", "/app.name:store/", "/applicationPool:store"]);
        assert_eq!(commands[3], ["set", "app", "/app.name:store/api", "/applicationPool:store"]);
        assert_eq!(commands[4], ["set", "vdir", "/vdir.name:store/", r"/physicalPath:C:\DNN\store"]);
        assert_eq!(rename_args(&site, "store", false, r"C:\s").len(), 2);
    }

    #[test]
    fn app_pool_sids_are_iis_own() {
        // The SID Windows shows for IIS APPPOOL\DefaultAppPool.
        assert_eq!(app_pool_sid("DefaultAppPool"), "S-1-5-82-3006700770-424185619-1745488364-794895919-4004696415");
        assert_eq!(app_pool_sid("defaultapppool"), app_pool_sid("DefaultAppPool"));
    }

    #[test]
    fn icacls_replaces_the_entries_by_sid() {
        let [remove, grant] = icacls_args(r"C:\DNN\shop", r"IIS APPPOOL\DefaultAppPool");
        let sid = "*S-1-5-82-3006700770-424185619-1745488364-794895919-4004696415";
        assert_eq!(remove, [r"C:\DNN\shop", "/remove", sid]);
        assert_eq!(grant, [r"C:\DNN\shop".to_string(), "/grant".into(), format!("{sid}:(OI)(CI)M")]);
        assert_eq!(icacls_args("C:\\a", "IIS_IUSRS")[1], ["C:\\a", "/grant", "*S-1-5-32-568:(OI)(CI)RX"]);
        assert_eq!(icacls_args("C:\\a", "iusr")[1], ["C:\\a", "/grant", "*S-1-5-17:(OI)(CI)RX"]);
        assert_eq!(icacls_args("C:\\a", r"PC\dev")[0], ["C:\\a", "/remove", r"PC\dev"]);
    }

    #[test]
    fn reads_sites_their_pools_and_who_shares_them() {
        let xml = r#"<appcmd>
            <SITE SITE.NAME="shop" SITE.ID="4" state="Started"><site name="shop" id="4">
                <application path="/" applicationPool="shop" /><application path="/api" /></site></SITE>
            <SITE SITE.NAME="blog" SITE.ID="5" state="Stopped"><site name="blog" id="5">
                <applicationDefaults applicationPool="blog" />
                <application path="/" /><application path="/x" applicationPool="Shop" /></site></SITE>
        </appcmd>"#;
        let sites = parse_site_entries(xml).unwrap();
        assert_eq!(sites[0].root_pool(), Some("shop"));
        assert_eq!(sites[0].apps[1], ("/api".to_string(), "DefaultAppPool".to_string()));
        assert_eq!(sites[1].root_pool(), Some("blog"));
        assert!(pool_used_by_others(&sites, "blog", "shop"));
        assert!(!pool_used_by_others(&sites, "blog", "blog"));
        assert!(pool_used_by_others(&sites, "SHOP", "shop"));
    }

    #[test]
    fn time_spans_both_ways() {
        assert_eq!(parse_timespan("1.05:00:00"), Some(Duration::from_secs(29 * 3600)));
        assert_eq!(parse_timespan("00:20:00"), Some(Duration::from_secs(1200)));
        assert_eq!(parse_timespan("00:00:01.5"), Some(Duration::from_millis(1500)));
        assert_eq!(parse_timespan("x"), None);
        assert_eq!(format_timespan(Duration::from_secs(29 * 3600)), "1.05:00:00");
        assert_eq!(format_timespan(Duration::ZERO), "00:00:00");
    }

    #[test]
    fn log_folder_is_the_sites_then_the_defaults() {
        assert_eq!(log_directory_from(SITES, "default web site"), Some(PathBuf::from(r"D:\logs\W3SVC1")));
        assert_eq!(log_directory_from(SITES, "shop"), Some(PathBuf::from(r"E:\iis\W3SVC4")));
        assert_eq!(log_directory_from(SITES, "gone"), None);
        let bare = "<sites><site name=\"a\" id=\"2\" /></sites>";
        assert_eq!(log_directory_from(bare, "a"), Some(expand_environment(DEFAULT_LOG_DIRECTORY).join("W3SVC2")));
    }

    #[test]
    fn reads_a_sites_details() {
        let site = site_config_from(SITES, "shop").unwrap();
        assert_eq!((site.id, site.server_auto_start, site.preload_enabled), (4, false, Some(true)));
        assert_eq!(site.root_pool.as_deref(), Some("shop"));
        assert_eq!(site.physical_path, PathBuf::from(r"C:\DNN\shop"));
        let https = &site.bindings[1];
        assert!(https.sni);
        assert_eq!(https.certificate_hash.as_deref(), Some("AB12CD"));
        assert_eq!(https.certificate_store.as_deref(), Some("WebHosting"));
        assert_eq!((site.bindings[2].sni, site.bindings[2].certificate_hash.as_ref()), (false, None));
        let default = site_config_from(SITES, "Default Web Site").unwrap();
        assert_eq!((default.server_auto_start, default.preload_enabled), (true, Some(false)));
        assert_eq!(default.root_pool.as_deref(), Some("DefaultAppPool"));
    }

    #[test]
    fn reads_a_pool_with_its_defaults() {
        let pool = pool_details_from(POOLS, "shop").unwrap();
        assert_eq!(pool.runtime, NO_MANAGED_CODE);
        assert_eq!((pool.identity_type.as_str(), pool.account.as_str()), ("SpecificUser", r"PC\dev"));
        assert_eq!((pool.start_mode.as_str(), pool.idle_timeout), ("AlwaysRunning", Duration::ZERO));
        assert_eq!(pool.recycle_interval, Duration::ZERO);
        assert_eq!(pool.recycle_at, [Duration::from_secs(3 * 3600), Duration::from_secs(15 * 3600 + 30 * 60)]);
        assert_eq!((pool.private_memory_limit_kb, pool.max_processes, pool.enable_32bit), (1_048_576, 2, true));
        assert!(!pool.rapid_fail_protection);
        // Not set on the pool: applicationPoolDefaults', then IIS's own default.
        assert!(pool.load_user_profile);
        assert_eq!(pool.rapid_fail_max_crashes, 5);

        let default = pool_details_from(POOLS, "defaultapppool").unwrap();
        assert_eq!((default.runtime.as_str(), default.pipeline.as_str()), ("v4.0", "Integrated"));
        assert_eq!(default.account, r"IIS APPPOOL\DefaultAppPool");
        assert_eq!(default.idle_timeout, Duration::from_secs(1200));
        assert_eq!(default.recycle_interval, Duration::from_secs(29 * 3600));
        assert!(pool_details_from(POOLS, "gone").is_none());
    }
}
