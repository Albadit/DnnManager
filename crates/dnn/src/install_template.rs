//! DNN's install template, `Install\DotNetNuke.install.config` (`src/DnnManager.Infrastructure/Dnn/DnnInstallTemplate.cs`):
//! what DNN's unattended install creates - the host account, the website and its alias. Written from the template DNN
//! ships (`DotNetNuke.install.config.resources`), so everything else stays as DNN has it.

use std::path::{Path, PathBuf};

use xot::Node;

use crate::install::DnnAccount;
use crate::safe_zip::ensure_no_link;
use crate::xml::SiteXml;

pub const FILE_NAME: &str = "DotNetNuke.install.config";
pub const SHIPPED_FILE_NAME: &str = "DotNetNuke.install.config.resources";

fn get_or_add(xml: &mut SiteXml, parent: Node, name: &str) -> Result<Node, String> {
    match xml.element(parent, name, "") {
        Some(node) => Ok(node),
        None => xml.append_element(parent, name, ""),
    }
}

fn set_child(xml: &mut SiteXml, parent: Node, name: &str, value: &str) -> Result<(), String> {
    let node = get_or_add(xml, parent, name)?;
    xml.set_text(node, value)
}

/// Writes the template into the site's Install folder and returns its path. Values go in through XML, so characters
/// like & and < can't break it. Leaves out DNN's <connection> node on purpose: with it DNN rewrites web.config's
/// connection string itself (and forces "User Instance" for a database file).
pub fn write(site_directory: &Path, account: &DnnAccount, portal_alias: &str) -> Result<PathBuf, String> {
    let install_directory = site_directory.join("Install");
    let shipped = install_directory.join(SHIPPED_FILE_NAME);
    if !shipped.is_file() {
        return Err("DNN's install template isn't in the package.".into());
    }
    let text = std::fs::read_to_string(&shipped).map_err(|e| format!("{}: {e}", shipped.display()))?;
    let mut xml = SiteXml::parse(&text)?;
    let root = xml.root().ok_or("DNN's install template is empty.")?;
    let (local, _) = xml.name_of(root).unwrap_or_default();
    if local != "dotnetnuke" {
        return Err(format!("DNN's install template starts with <{local}>, not <dotnetnuke>."));
    }

    // Read as "is it en-US?": a missing node would have DNN remove en-US from the site.
    set_child(&mut xml, root, "installCulture", &account.language)?;

    let superuser = get_or_add(&mut xml, root, "superuser")?;
    set_child(&mut xml, superuser, "username", &account.user_name)?;
    set_child(&mut xml, superuser, "password", &account.password)?;
    set_child(&mut xml, superuser, "email", &account.email)?;
    set_child(&mut xml, superuser, "locale", &account.language)?;
    set_child(&mut xml, superuser, "updatepassword", "false")?;

    let settings = get_or_add(&mut xml, root, "settings")?;
    set_child(&mut xml, settings, "HostEmail", &account.email)?;
    for connection in xml.elements(root, "connection", "") {
        xml.xot.remove(connection).map_err(|e| e.to_string())?;
    }

    let portals = get_or_add(&mut xml, root, "portals")?;
    let existing = xml.elements(portals, "portal", "");
    let portal = match existing.first() {
        Some(portal) => *portal,
        None => xml.append_element(portals, "portal", "")?,
    };
    for extra in existing.iter().skip(1) {
        xml.xot.remove(*extra).map_err(|e| e.to_string())?;
    }
    set_child(&mut xml, portal, "portalname", &account.website_name)?;
    // DNN needs the node to create the site; since DNN 9.3 the host becomes the site's administrator and these
    // credentials aren't used - a random password keeps the shipped default out of it.
    let administrator = get_or_add(&mut xml, portal, "administrator")?;
    if xml.element(administrator, "username", "").is_none() {
        set_child(&mut xml, administrator, "username", "admin")?;
    }
    set_child(&mut xml, administrator, "password", &random_password())?;
    set_child(&mut xml, portal, "templatefile", &format!("{}.template", account.template))?;
    let aliases = get_or_add(&mut xml, portal, "portalaliases")?;
    xml.set_text(aliases, "")?;
    let alias = xml.append_element(aliases, "portalalias", "")?;
    xml.set_text(alias, portal_alias)?;
    set_child(&mut xml, portal, "ischild", "false")?;

    let target = install_directory.join(FILE_NAME);
    // The host password goes in it: never through a link or junction in the site to a file elsewhere, nor into a file
    // with other names (a hard link) - a file there is replaced, not written into.
    ensure_no_link(site_directory, &target, None)?;
    if target.exists() {
        std::fs::remove_file(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    std::fs::write(&target, xml.to_text()?).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(target)
}

/// Deletes the written template and DNN's shipped one - both are read by an installer that asks no password.
pub fn delete(site_directory: &Path) {
    for name in [FILE_NAME, SHIPPED_FILE_NAME] {
        let path = site_directory.join("Install").join(name);
        if path.is_file() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// `Convert.ToBase64String(RandomNumberGenerator.GetBytes(24))`.
fn random_password() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).expect("the system's random number generator");
    bytes
        .chunks(3)
        .flat_map(|c| {
            let n = (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32;
            (0..4).rev().map(move |i| ALPHABET[(n >> (6 * i) & 63) as usize] as char)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_template_has_the_account_the_alias_and_no_connection() {
        let dir = tempfile::tempdir().unwrap();
        let site = dir.path();
        std::fs::create_dir_all(site.join("Install")).unwrap();
        std::fs::write(
            site.join("Install").join(SHIPPED_FILE_NAME),
            "<dotnetnuke><version>10.00.00</version><installCulture>en-US</installCulture>\
             <connection><server>(local)</server><database>x</database></connection>\
             <superuser><username>host</username><password>dnnhost</password><email>host@changeme.invalid</email></superuser>\
             <settings><HostEmail></HostEmail></settings>\
             <portals><portal><portalname>My Website</portalname><administrator><username>admin</username><password>dnnadmin</password></administrator>\
             <templatefile>Default Website.template</templatefile><portalaliases><portalalias></portalalias></portalaliases><ischild>false</ischild></portal></portals></dotnetnuke>",
        )
        .unwrap();
        let account = DnnAccount {
            user_name: "sitehost".into(),
            password: "pa<ss>&'\"word".into(),
            email: "me@example.com".into(),
            website_name: "Shop & Co <test>".into(),
            language: "en-US".into(),
            template: "Blank Website".into(),
        };
        let written = std::fs::read_to_string(write(site, &account, "shop.dnndev.me").unwrap()).unwrap();
        let doc = roxmltree::Document::parse(&written).unwrap();
        let root = doc.root_element();
        fn child<'a, 'i>(parent: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
            parent.children().find(|n| n.has_tag_name(name))
        }
        let text = |parent, path: &[&str]| {
            let mut node = parent;
            for name in path {
                node = child(node, name).unwrap();
            }
            node.text().unwrap_or_default().to_string()
        };
        assert!(child(root, "connection").is_none(), "<connection> would make DNN rewrite web.config.");
        assert_eq!(text(root, &["superuser", "username"]), "sitehost");
        assert_eq!(text(root, &["superuser", "password"]), account.password);
        assert_eq!(text(root, &["superuser", "updatepassword"]), "false");
        assert_eq!(text(root, &["settings", "HostEmail"]), "me@example.com");
        assert_eq!(text(root, &["portals", "portal", "portalname"]), "Shop & Co <test>");
        assert_eq!(text(root, &["portals", "portal", "templatefile"]), "Blank Website.template");
        assert_eq!(text(root, &["portals", "portal", "portalaliases", "portalalias"]), "shop.dnndev.me");
        assert_ne!(text(root, &["portals", "portal", "administrator", "password"]), "dnnadmin");

        delete(site);
        assert_eq!(std::fs::read_dir(site.join("Install")).unwrap().count(), 0, "The templates are still there.");
    }

    #[test]
    fn a_template_that_isnt_dnns_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Install")).unwrap();
        let account = DnnAccount {
            user_name: "h".into(),
            password: "p".into(),
            email: "e".into(),
            website_name: "w".into(),
            language: "en-US".into(),
            template: "t".into(),
        };
        assert!(write(dir.path(), &account, "a").unwrap_err().contains("isn't in the package"));
        std::fs::write(dir.path().join("Install").join(SHIPPED_FILE_NAME), "<other />").unwrap();
        assert_eq!(
            write(dir.path(), &account, "a").unwrap_err(),
            "DNN's install template starts with <other>, not <dotnetnuke>."
        );
        assert_eq!(random_password().len(), 32);
    }
}
