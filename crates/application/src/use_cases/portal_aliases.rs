//! DNN's portal aliases between the live site and this PC. A site brought here (a backup restored, a clone) gets a
//! local address for every portal - portal 0 the site's own, every other portal one under it
//! (`projectflip.goededoelen.dnndev.me`) - made primary, with the live aliases kept as the portal's other aliases and
//! the live primary one noted in the portal's settings ([`LIVE_PRIMARY_SETTING`]). Exported for the live server, the
//! local aliases go again and the noted ones are primary once more: the database works there as it is.
//!
//! The rules here are pure; the SQL that reads and writes the aliases is the `sql` crate's.

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// The portal setting that keeps a portal's live primary alias while it is primary here under a local one.
pub const LIVE_PRIMARY_SETTING: &str = "DnnManager_LivePrimaryAlias";

/// One row of DNN's PortalAlias table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasRow {
    pub id: i64,
    pub portal: i32,
    /// `host[:port][/path]`, as DNN keeps it.
    pub alias: String,
    pub primary: bool,
}

/// An alias's host: without its path and its port, in lower case.
pub fn host_of(alias: &str) -> String {
    let host = alias.split('/').next().unwrap_or_default();
    host.split(':').next().unwrap_or_default().trim().to_ascii_lowercase()
}

/// Whether the alias has a path - a child portal's (`example.com/shop`).
pub fn has_path(alias: &str) -> bool {
    alias.trim_end_matches('/').contains('/')
}

/// Whether `alias` is one of this PC's: its host is `localhost`, the host-name `suffix` or a name under it, or one of
/// `local_hosts` (the site's own) or a name under one.
pub fn is_local(alias: &str, suffix: &str, local_hosts: &[String]) -> bool {
    let host = host_of(alias);
    let under = |name: &str| {
        let name = name.trim().trim_matches('.').to_ascii_lowercase();
        !name.is_empty() && (host == name || host.ends_with(&format!(".{name}")))
    };
    under("localhost") || under(suffix) || local_hosts.iter().any(|h| under(&host_of(h)))
}

/// The name a portal's local address starts with, from one of its aliases: the host's first label that isn't `www`,
/// in lower case, letters, digits and dashes only.
pub fn slug(alias: &str) -> Option<String> {
    let host = host_of(alias);
    let label = host.split('.').find(|l| !l.is_empty() && *l != "www")?;
    let slug: String = label.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    let slug = slug.trim_matches('-').to_string();
    (!slug.is_empty()).then_some(slug)
}

/// What a portal's aliases become here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Localized {
    pub portal: i32,
    /// Its local address - primary.
    pub alias: String,
    /// The local alias renamed to it (one from another address of this PC, a clone's or an earlier one); None: a new
    /// row, or it is there already ([`Localized::present`]).
    pub rewrite: Option<i64>,
    /// It is there already.
    pub present: bool,
    /// The portal's other local aliases without a path - from another address of this PC - removed.
    pub remove: Vec<i64>,
    /// The live alias that was primary, to note; None: none was (it is noted already, or the site never was live).
    pub live_primary: Option<String>,
}

/// Every portal's local address, for a site that answers here at `site_alias` (`host[:port]`); `portals` are the
/// portals the database has (one without an alias gets one too). `suffix` is the settings' host-name suffix: aliases
/// under it are this PC's, as are those under the site's host.
pub fn localize(rows: &[AliasRow], portals: &[i32], suffix: &str, site_alias: &str) -> Vec<Localized> {
    let site_alias = site_alias.trim().to_ascii_lowercase();
    let site_host = host_of(&site_alias);
    let port = site_alias.split('/').next().unwrap_or_default().split_once(':').map(|(_, p)| format!(":{p}"));
    let local_hosts = [site_host.clone()];
    let ids: BTreeSet<i32> = portals.iter().copied().chain(rows.iter().map(|r| r.portal)).collect();
    let taken: HashMap<String, i32> = rows.iter().map(|r| (r.alias.to_ascii_lowercase(), r.portal)).collect();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for portal in ids {
        let mine: Vec<&AliasRow> = rows.iter().filter(|r| r.portal == portal).collect();
        let (local, live): (Vec<&AliasRow>, Vec<&AliasRow>) =
            mine.iter().partition(|r| is_local(&r.alias, suffix, &local_hosts));
        let live_primary = live.iter().find(|r| r.primary).map(|r| r.alias.clone());
        let alias = if portal == 0 {
            site_alias.clone()
        } else {
            let base = live_primary
                .as_deref()
                .and_then(slug)
                .or_else(|| live.iter().filter(|r| !has_path(&r.alias)).find_map(|r| slug(&r.alias)))
                .or_else(|| local.iter().filter(|r| !has_path(&r.alias)).find_map(|r| slug(&r.alias)))
                .unwrap_or_else(|| format!("portal{portal}"));
            let mut name = base.clone();
            let clashes = |name: &str| {
                let alias = format!("{name}.{site_host}{}", port.as_deref().unwrap_or_default());
                used.contains(name) || taken.get(&alias).is_some_and(|p| *p != portal)
            };
            if clashes(&name) {
                name = format!("{base}-{portal}");
            }
            used.insert(name.clone());
            format!("{name}.{site_host}{}", port.as_deref().unwrap_or_default())
        };
        let renamable: Vec<&&AliasRow> = local.iter().filter(|r| !has_path(&r.alias)).collect();
        let present = renamable.iter().any(|r| r.alias.eq_ignore_ascii_case(&alias));
        let rewrite = if present { None } else { renamable.first().map(|r| r.id) };
        let remove = renamable
            .iter()
            .filter(|r| !r.alias.eq_ignore_ascii_case(&alias) && Some(r.id) != rewrite)
            .map(|r| r.id)
            .collect();
        out.push(Localized { portal, alias, rewrite, present, remove, live_primary });
    }
    out
}

/// What a portal's aliases become for the live server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delocalized {
    pub portal: i32,
    /// Its local aliases, removed.
    pub remove: Vec<i64>,
    /// The alias made primary; None: the portal has no live alias left.
    pub primary: Option<i64>,
}

impl Delocalized {
    /// The live alias made primary, by name.
    pub fn primary_alias<'a>(&self, rows: &'a [AliasRow]) -> Option<&'a str> {
        let id = self.primary?;
        rows.iter().find(|r| r.id == id).map(|r| r.alias.as_str())
    }
}

/// Every portal's aliases for the live server: the local ones (under `suffix`, or one of `local_hosts`) removed, and
/// primary the one noted in `noted` (the portal's [`LIVE_PRIMARY_SETTING`]) - else the one primary already, else the
/// oldest.
pub fn delocalize(
    rows: &[AliasRow],
    noted: &HashMap<i32, String>,
    suffix: &str,
    local_hosts: &[String],
) -> Vec<Delocalized> {
    let mut portals: BTreeMap<i32, Vec<&AliasRow>> = BTreeMap::new();
    for r in rows {
        portals.entry(r.portal).or_default().push(r);
    }
    portals
        .into_iter()
        .map(|(portal, mine)| {
            let (local, live): (Vec<&AliasRow>, Vec<&AliasRow>) =
                mine.into_iter().partition(|r| is_local(&r.alias, suffix, local_hosts));
            let primary = noted
                .get(&portal)
                .and_then(|n| live.iter().find(|r| r.alias.eq_ignore_ascii_case(n.trim())))
                .or_else(|| live.iter().find(|r| r.primary))
                .or_else(|| live.iter().min_by_key(|r| r.id))
                .map(|r| r.id);
            Delocalized { portal, remove: local.iter().map(|r| r.id).collect(), primary }
        })
        .collect()
}

/// Whether `plan` changes anything: a local alias to remove, a primary to move, or a noted live primary to clear.
pub fn changes(rows: &[AliasRow], noted: &HashMap<i32, String>, plan: &[Delocalized]) -> bool {
    !noted.is_empty()
        || plan.iter().any(|d| {
            !d.remove.is_empty() || d.primary.is_some_and(|id| rows.iter().any(|r| r.id == id && !r.primary))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: i64, portal: i32, alias: &str, primary: bool) -> AliasRow {
        AliasRow { id, portal, alias: alias.into(), primary }
    }

    /// The site in the screenshot: a live backup of six portals, one with a local alias someone added by hand.
    fn live_site() -> Vec<AliasRow> {
        vec![
            row(1, 0, "goededoelen.bondtest.nl", true),
            row(2, 0, "community.bondtest.nl", false),
            row(3, 1, "projectflip.bondtest.nl", true),
            row(4, 2, "sngv.bondtest.nl", true),
            row(5, 4, "grootvlaardingsdictee.bondtest.nl", true),
            row(6, 4, "grootvlaardingsdictee.dnndev.me", false),
            row(7, 4, "goededoelen.dnndev.me/grootvlaardingsdictee", false),
            row(8, 5, "rotaryvlaardingen.bondtest.nl", false),
            row(9, 5, "www.rotary-vlaardingen.nl", true),
        ]
    }

    #[test]
    fn local_aliases_are_known_by_their_host() {
        let site = ["shop.test:8080".to_string()];
        assert!(is_local("shop.dnndev.me", "dnndev.me", &[]));
        assert!(is_local("x.shop.dnndev.me:8080/child", "dnndev.me", &[]));
        assert!(is_local("localhost/dnn", "dnndev.me", &[]));
        assert!(is_local("portal.shop.test", "dnndev.me", &site));
        assert!(!is_local("shop.nl", "dnndev.me", &site));
        assert!(!is_local("notdnndev.me", "dnndev.me", &[]));
        assert_eq!(slug("www.Rotary-Vlaardingen.nl:443/x"), Some("rotary-vlaardingen".into()));
        assert_eq!(slug("www"), None);
    }

    #[test]
    fn every_portal_gets_a_local_primary_and_keeps_its_live_aliases() {
        let plan = localize(&live_site(), &[0, 1, 2, 3, 4, 5], "dnndev.me", "goededoelen.dnndev.me");
        let by = |p: i32| plan.iter().find(|l| l.portal == p).unwrap();
        assert_eq!(by(0).alias, "goededoelen.dnndev.me");
        assert_eq!((by(0).rewrite, by(0).live_primary.as_deref()), (None, Some("goededoelen.bondtest.nl")));
        assert_eq!(by(1).alias, "projectflip.goededoelen.dnndev.me");
        assert_eq!(by(3).alias, "portal3.goededoelen.dnndev.me", "a portal without an alias");
        assert_eq!(by(3).live_primary, None);
        // The hand-made local alias is renamed; the child-portal path alias is left alone.
        assert_eq!(by(4).alias, "grootvlaardingsdictee.goededoelen.dnndev.me");
        assert_eq!((by(4).rewrite, by(4).remove.clone()), (Some(6), vec![]));
        // Named after the live primary alias, not the first.
        assert_eq!(by(5).alias, "rotary-vlaardingen.goededoelen.dnndev.me");
        assert_eq!(by(5).live_primary.as_deref(), Some("www.rotary-vlaardingen.nl"));
    }

    #[test]
    fn a_clone_moves_the_local_aliases_to_its_own_address_on_its_port() {
        let rows = vec![
            row(1, 0, "goededoelen.bondtest.nl", false),
            row(2, 0, "goededoelen.dnndev.me", true),
            row(3, 1, "projectflip.bondtest.nl", false),
            row(4, 1, "projectflip.goededoelen.dnndev.me", true),
            row(5, 1, "old.dnndev.me", false),
        ];
        let plan = localize(&rows, &[0, 1], "dnndev.me", "copy.dnndev.me:8080");
        assert_eq!((plan[0].alias.as_str(), plan[0].rewrite), ("copy.dnndev.me:8080", Some(2)));
        assert_eq!(plan[0].live_primary, None, "the live primary is noted already");
        assert_eq!((plan[1].alias.as_str(), plan[1].rewrite, plan[1].remove.clone()), ("projectflip.copy.dnndev.me:8080", Some(4), vec![5]));
        // Brought here again at the same address: nothing to rename.
        let again = localize(&rows, &[0, 1], "dnndev.me", "goededoelen.dnndev.me");
        assert!(again[0].present && again[1].present && again[1].rewrite.is_none());
    }

    #[test]
    fn two_portals_with_the_same_name_get_different_addresses() {
        let rows = vec![row(1, 1, "shop.a.nl", true), row(2, 2, "shop.b.nl", true)];
        let plan = localize(&rows, &[], "dnndev.me", "site.dnndev.me");
        assert_eq!(plan[0].alias, "shop.site.dnndev.me");
        assert_eq!(plan[1].alias, "shop-2.site.dnndev.me");
    }

    #[test]
    fn for_the_live_server_the_local_aliases_go_and_the_noted_primary_is_back() {
        let mut rows = live_site();
        for r in &mut rows {
            r.primary = false;
        }
        rows.push(row(10, 0, "goededoelen.dnndev.me", true));
        rows.push(row(11, 1, "projectflip.goededoelen.dnndev.me", true));
        rows.push(row(12, 3, "portal3.goededoelen.dnndev.me", true));
        let noted: HashMap<i32, String> =
            [(0, "goededoelen.bondtest.nl".into()), (5, "WWW.rotary-vlaardingen.nl".into())].into();
        let plan = delocalize(&rows, &noted, "dnndev.me", &["goededoelen.dnndev.me".into()]);
        let by = |p: i32| plan.iter().find(|d| d.portal == p).unwrap();
        assert_eq!((by(0).remove.clone(), by(0).primary_alias(&rows)), (vec![10], Some("goededoelen.bondtest.nl")));
        assert_eq!(by(1).primary_alias(&rows), Some("projectflip.bondtest.nl"), "nothing noted: the oldest");
        assert_eq!((by(3).remove.clone(), by(3).primary), (vec![12], None), "made here: no live alias");
        assert_eq!(by(4).remove, vec![6, 7]);
        assert_eq!(by(5).primary_alias(&rows), Some("www.rotary-vlaardingen.nl"));
        assert!(changes(&rows, &noted, &plan));
        // A site that never came here: nothing to change.
        let live = vec![row(1, 0, "shop.nl", true), row(2, 0, "www.shop.nl", false)];
        let plan = delocalize(&live, &HashMap::new(), "dnndev.me", &[]);
        assert!(!changes(&live, &HashMap::new(), &plan));
    }
}
