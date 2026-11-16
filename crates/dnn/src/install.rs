//! What belongs with DNN's unattended install (`src/DnnManager.Application/Abstractions/DnnInstall.cs`): how a project's
//! DNN is installed, the site an install talks to, the host account and website it creates, and DNN's rules for them.

/// How a new project's DNN gets installed (`DnnInstallMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnnInstallMode {
    /// DNN Manager runs DNN's own unattended install: the first visit shows the new site.
    Automatic,
    /// DNN's installation wizard, filled in by the user on the first visit - DNN's normal behaviour.
    Manual,
}

/// The site an automatic install talks to: requests go to this machine on `port` with `alias` as the host name - the IIS
/// binding routes them - so the install doesn't depend on the host name resolving (`DnnSiteAddress`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnSiteAddress {
    /// The site's folder: web.config, bin, Install.
    pub directory: std::path::PathBuf,
    /// The `host[:port]` the site answers on - it becomes the portal alias, e.g. `mysite.dnndev.me`.
    pub alias: String,
    /// The port the site's binding listens on.
    pub port: u16,
}

impl DnnSiteAddress {
    /// The site's address for people: `http://mysite.dnndev.me/`.
    pub fn url(&self) -> String {
        format!("http://{}/", self.alias)
    }

    /// The portal alias for a host name and port: the port only when it isn't 80.
    pub fn alias_for(host_name: &str, port: u16) -> String {
        if port == 80 { host_name.to_string() } else { format!("{host_name}:{port}") }
    }
}

/// The host account and website an automatic install creates (`DnnAccount`).
#[derive(Clone, PartialEq, Eq)]
pub struct DnnAccount {
    pub user_name: String,
    pub password: String,
    pub email: String,
    pub website_name: String,
    pub language: String,
    pub template: String,
}

impl std::fmt::Debug for DnnAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DnnAccount")
            .field("user_name", &self.user_name)
            .field("email", &self.email)
            .field("website_name", &self.website_name)
            .field("language", &self.language)
            .field("template", &self.template)
            .finish_non_exhaustive()
    }
}

/// The `minRequiredPasswordLength` of DNN's shipped membership provider.
pub const MIN_PASSWORD_LENGTH: usize = 7;
/// The longest password ASP.NET's membership provider accepts.
pub const MAX_PASSWORD_LENGTH: usize = 128;
/// DNN's extension packages have deep folders: from about 100 characters on, some of them fail to install (Windows'
/// 260-character path limit) while DNN still reports a complete installation.
pub const MAX_SITE_PATH_LENGTH: usize = 100;
/// The cultures DNN's installer offers. Anything but en-US has DNN download its language pack during the install.
pub const LANGUAGES: [&str; 6] = ["en-US", "de-DE", "es-ES", "fr-FR", "it-IT", "nl-NL"];
/// The site templates DNN's install package ships (`Portals\_default\*.template`).
pub const TEMPLATES: [&str; 2] = ["Default Website", "Blank Website"];

/// `DnnAccountRules.UserNameProblem`.
pub fn user_name_problem(user_name: &str) -> Option<String> {
    if user_name.trim().is_empty() {
        return Some("Enter the host account's user name.".into());
    }
    if user_name.encode_utf16().count() > 100 {
        return Some("The user name can have at most 100 characters.".into());
    }
    (!user_name.chars().all(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | '@')))
        .then(|| "The user name can only have letters, digits and . _ - @.".into())
}

/// `DnnAccountRules.PasswordProblem`.
pub fn password_problem(password: &str) -> Option<String> {
    let length = password.encode_utf16().count();
    if length < MIN_PASSWORD_LENGTH {
        Some(format!("The password needs at least {MIN_PASSWORD_LENGTH} characters."))
    } else if length > MAX_PASSWORD_LENGTH {
        Some(format!("The password can have at most {MAX_PASSWORD_LENGTH} characters."))
    } else if password.contains('<') || password.contains("&#") {
        // ASP.NET's request validation refuses these in DNN's login form - such a password could never sign in.
        Some("The password can't contain < or &# - DNN's login form refuses them.".into())
    } else {
        None
    }
}

/// A strong random password DNN's login form accepts: letters, digits and a few symbols, no < or &#
/// (`DnnAccountRules.GeneratePassword`).
pub fn generate_password(length: usize) -> String {
    const LETTERS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    const DIGITS: &[u8] = b"23456789";
    const SYMBOLS: &[u8] = b"!%*-_=+.";
    let all: Vec<u8> = [LETTERS, DIGITS, SYMBOLS].concat();
    let mut chars = vec![
        LETTERS[random_below(24)],
        LETTERS[24 + random_below(LETTERS.len() - 24)],
        DIGITS[random_below(DIGITS.len())],
        SYMBOLS[random_below(SYMBOLS.len())],
    ];
    while chars.len() < length {
        chars.push(all[random_below(all.len())]);
    }
    for i in (1..chars.len()).rev() {
        chars.swap(i, random_below(i + 1));
    }
    String::from_utf8(chars).expect("ASCII")
}

/// A uniform random number below `n`, from the system's cryptographic generator (`RandomNumberGenerator.GetInt32`).
fn random_below(n: usize) -> usize {
    let n = n as u64;
    let zone = u64::MAX - u64::MAX % n;
    loop {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("the system's random number generator");
        let value = u64::from_le_bytes(bytes);
        if value < zone {
            return (value % n) as usize;
        }
    }
}

/// `DnnAccountRules.EmailProblem`.
pub fn email_problem(email: &str) -> Option<String> {
    let ok = match email.find('@') {
        Some(at) => {
            let domain = &email[at + 1..];
            at > 0
                && !domain.is_empty()
                && !domain.contains('@')
                && !email.chars().any(char::is_whitespace)
                && domain.contains('.')
        }
        None => false,
    };
    (!ok).then(|| "Enter an e-mail address, e.g. host@example.com.".into())
}

/// `DnnAccountRules.WebsiteNameProblem`.
pub fn website_name_problem(name: &str) -> Option<String> {
    if name.trim().is_empty() {
        Some("Enter the website's name.".into())
    } else if name.encode_utf16().count() > 128 {
        Some("The website's name can have at most 128 characters.".into())
    } else {
        None
    }
}

/// Everything wrong with `account`, empty when DNN can install it (`DnnAccountRules.Problems`).
pub fn account_problems(account: &DnnAccount) -> Vec<String> {
    [
        user_name_problem(&account.user_name),
        password_problem(&account.password),
        email_problem(&account.email),
        website_name_problem(&account.website_name),
        (!LANGUAGES.contains(&account.language.as_str()))
            .then(|| format!("DNN doesn't install in '{}'.", account.language)),
        (!TEMPLATES.contains(&account.template.as_str()))
            .then(|| format!("DNN has no site template '{}'.", account.template)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(language: &str, template: &str) -> DnnAccount {
        DnnAccount {
            user_name: "host".into(),
            password: "abcdefg".into(),
            email: "h@x.io".into(),
            website_name: "Site".into(),
            language: language.into(),
            template: template.into(),
        }
    }

    #[test]
    fn account_rules_refuse_what_dnn_would_fail_on_silently() {
        assert!(password_problem("abc").is_some(), "DNN finishes without a host for a 3-character password.");
        assert!(password_problem("abcdefg").is_none());
        assert!(password_problem("abc<defg").is_some());
        assert!(user_name_problem("host,admin").is_some());
        assert!(user_name_problem("").is_some());
        assert!(user_name_problem("host").is_none());
        assert!(email_problem("host").is_some());
        assert!(email_problem("host@dnndev.me").is_none());
        assert!(account_problems(&account("en-US", "Default Website")).is_empty());
        assert_eq!(account_problems(&account("xx-XX", "Other")).len(), 2);
        assert!(!format!("{:?}", account("en-US", "x")).contains("abcdefg"), "The password is never printed.");
    }

    #[test]
    fn a_generated_password_passes_the_rules() {
        for _ in 0..50 {
            let password = generate_password(16);
            assert_eq!(password.len(), 16);
            assert!(password_problem(&password).is_none(), "{password}");
            assert!(password.chars().any(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn the_alias_has_the_port_only_when_it_isnt_80() {
        assert_eq!(DnnSiteAddress::alias_for("shop.dnndev.me", 80), "shop.dnndev.me");
        assert_eq!(DnnSiteAddress::alias_for("localhost", 8081), "localhost:8081");
    }
}
