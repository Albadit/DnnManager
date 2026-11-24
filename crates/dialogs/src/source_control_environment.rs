//! Add or edit a Source Control environment: its name and kind, the SFTP server (host, port, user, password or key
//! file), the site's folder there, and - one line each, `;` between items - the mappings (`local => /remote`), the
//! ignore rules and the protected paths; and whether the server's files are backed up before they are replaced.
//! Emits `DialogEvent::Done(EnvironmentEdit)`; the app saves it (`dnnm_wiring::source_control::save_environment`).
//!
//! The password (or the key's passphrase) is never shown: left empty, the saved one stays.

use dnnm_ui::px;

use gpui::{ClickEvent, Context, Entity, EventEmitter, Focusable, KeyDownEvent, SharedString, Subscription, Window, div, prelude::*};

use dnnm_application::source_control::model::{Auth, Environment, EnvironmentKind, Mapping, Protocol};
use dnnm_application::source_control::paths::clean;
use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::{self as ui, Tip};

use crate::host_password::{PasswordField, PasswordFieldChanged};

/// What the dialog gives back: the environment, and its secret when one was typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentEdit {
    pub env: Environment,
    pub secret: Option<String>,
}

/// The form as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Form {
    pub protocol: Protocol,
    pub name: String,
    pub host: String,
    pub port: String,
    pub user: String,
    pub key_file: Option<String>,
    pub remote_root: String,
    pub mappings: String,
    pub ignore: String,
    pub protected: String,
}

/// `a; b;; c` as its items.
pub fn items(text: &str) -> Vec<String> {
    text.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// `DesktopModules/Shop => /srv/shop; …` as mappings - or what is wrong with them.
pub fn parse_mappings(text: &str) -> Result<Vec<Mapping>, String> {
    items(text)
        .into_iter()
        .map(|item| {
            let (local, remote) = item
                .split_once("=>")
                .ok_or_else(|| format!("'{item}': write it as a project folder => a server folder"))?;
            let local = clean(local.trim()).ok_or_else(|| format!("'{}' leaves the project", local.trim()))?;
            let remote = remote.trim();
            if !remote.starts_with('/') {
                return Err(format!("'{remote}': the server folder must be a full path, starting with /"));
            }
            Ok(Mapping { local, remote: remote.trim_end_matches('/').to_string().max("/".into()) })
        })
        .collect()
}

/// What is wrong with the form; None when it can be saved. `has_secret`: a password is saved already.
pub fn problem(form: &Form, has_secret: bool, typed_secret: bool) -> Option<String> {
    let host = form.host.trim();
    if form.name.trim().is_empty() {
        return Some("Name the environment.".into());
    }
    if host.is_empty() || host.contains(char::is_whitespace) || host.contains('/') {
        return Some("Enter the server's host name or address, without spaces or a path.".into());
    }
    if !form.port.trim().parse::<u16>().is_ok_and(|p| p > 0) {
        return Some("The port is a number from 1 to 65535 (SFTP's is 22).".into());
    }
    if form.user.trim().is_empty() {
        return Some("Enter the user to sign in as.".into());
    }
    match &form.key_file {
        Some(_) if form.protocol != Protocol::Sftp => return Some("A key file signs in to SFTP only - use a password.".into()),
        Some(path) if path.trim().is_empty() => return Some("Enter the path of the private key file.".into()),
        None if !has_secret && !typed_secret => return Some("Enter the password.".into()),
        _ => {}
    }
    if !form.remote_root.trim().starts_with('/') {
        return Some("The site's folder on the server is a full path, starting with / (e.g. /var/www/shop).".into());
    }
    if let Err(e) = parse_mappings(&form.mappings) {
        return Some(e);
    }
    if let Some(bad) = items(&form.ignore).into_iter().chain(items(&form.protected)).find(|p| p.contains("..")) {
        return Some(format!("'{bad}': a rule can't climb out of the project with .."));
    }
    None
}

/// A new environment's id: its project and a number of its own.
pub fn new_id(project: &str) -> String {
    let slug: String = project.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    format!("{slug}-{:x}", nanos & 0xFFFF_FFFF_FFFF)
}

/// The environment the form describes, keeping what the form doesn't hold (its id, the trusted key).
pub fn environment(form: &Form, kind: EnvironmentKind, backup: bool, base: &Environment) -> Environment {
    Environment {
        name: form.name.trim().into(),
        kind,
        protocol: form.protocol,
        host: form.host.trim().into(),
        port: form.port.trim().parse().unwrap_or(22),
        user: form.user.trim().into(),
        auth: form.key_file.as_ref().map_or(Auth::Password, |k| Auth::KeyFile(k.trim().into())),
        remote_root: form.remote_root.trim().trim_end_matches('/').to_string().max("/".into()),
        mappings: parse_mappings(&form.mappings).unwrap_or_default(),
        ignore: items(&form.ignore),
        protected: items(&form.protected),
        backup_remote: backup,
        ..base.clone()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Host,
    Port,
    User,
    KeyFile,
    RemoteRoot,
    Mappings,
    Ignore,
    Protected,
}

pub struct EnvironmentDialog {
    base: Environment,
    is_new: bool,
    has_secret: bool,
    kind: EnvironmentKind,
    kind_open: bool,
    protocol: Protocol,
    protocol_open: bool,
    use_key: bool,
    backup: bool,
    inputs: Vec<(Field, Entity<TextInput>)>,
    secret: Entity<PasswordField>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent<EnvironmentEdit>> for EnvironmentDialog {}

impl EnvironmentDialog {
    /// For `env` - a new one when `is_new` (its fields as given: the project's defaults). `has_secret`: a password is
    /// saved for it.
    pub fn new(env: Environment, is_new: bool, has_secret: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mappings = env.mappings.iter().map(|m| format!("{} => {}", m.local, m.remote)).collect::<Vec<_>>().join("; ");
        let key_file = match &env.auth {
            Auth::KeyFile(path) => path.clone(),
            Auth::Password => String::new(),
        };
        let values = [
            (Field::Name, env.name.clone(), ""),
            (Field::Host, env.host.clone(), "sftp.example.com"),
            (Field::Port, env.port.to_string(), "22"),
            (Field::User, env.user.clone(), "deploy"),
            (Field::KeyFile, key_file, "~/.ssh/id_ed25519"),
            (Field::RemoteRoot, env.remote_root.clone(), "/var/www/shop"),
            (Field::Mappings, mappings, "DesktopModules/Shop => /var/www/shop/DesktopModules/Shop"),
            (Field::Ignore, env.ignore.join("; "), "*.map; Portals/0/Temp/"),
            (Field::Protected, env.protected.join("; "), "robots.txt; Portals/0/Documents/"),
        ];
        let mut subscriptions = Vec::new();
        let inputs: Vec<(Field, Entity<TextInput>)> = values
            .into_iter()
            .map(|(field, value, placeholder)| {
                let input = cx.new(|cx| {
                    let mut input = TextInput::new(placeholder, cx);
                    input.set_text(value, cx);
                    input
                });
                subscriptions.push(cx.subscribe(&input, |_, _, _: &TextChanged, cx| cx.notify()));
                (field, input)
            })
            .collect();
        let secret = cx.new(PasswordField::new);
        subscriptions.push(cx.subscribe(&secret, |_, _, _: &PasswordFieldChanged, cx| cx.notify()));
        window.focus(&inputs[0].1.read(cx).focus_handle(cx));
        Self {
            kind: env.kind,
            use_key: matches!(env.auth, Auth::KeyFile(_)),
            protocol: env.protocol,
            protocol_open: false,
            backup: env.backup_remote,
            base: env,
            is_new,
            has_secret,
            kind_open: false,
            inputs,
            secret,
            _subscriptions: subscriptions,
        }
    }

    fn text(&self, field: Field, cx: &gpui::App) -> String {
        self.inputs.iter().find(|(f, _)| *f == field).map(|(_, i)| i.read(cx).text().to_string()).unwrap_or_default()
    }

    fn input(&self, field: Field) -> &Entity<TextInput> {
        &self.inputs.iter().find(|(f, _)| *f == field).expect("every field has its input").1
    }

    fn form(&self, cx: &gpui::App) -> Form {
        Form {
            protocol: self.protocol,
            name: self.text(Field::Name, cx),
            host: self.text(Field::Host, cx),
            port: self.text(Field::Port, cx),
            user: self.text(Field::User, cx),
            key_file: (self.use_key && self.protocol == Protocol::Sftp).then(|| self.text(Field::KeyFile, cx)),
            remote_root: self.text(Field::RemoteRoot, cx),
            mappings: self.text(Field::Mappings, cx),
            ignore: self.text(Field::Ignore, cx),
            protected: self.text(Field::Protected, cx),
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let form = self.form(cx);
        let typed = self.secret.read(cx).password(cx);
        if problem(&form, self.has_secret, !typed.is_empty()).is_some() {
            return;
        }
        let env = environment(&form, self.kind, self.backup, &self.base);
        cx.emit(DialogEvent::Done(EnvironmentEdit { env, secret: (!typed.is_empty()).then_some(typed) }));
    }

    /// Another protocol: its default port when the port was the old one's.
    fn choose_protocol(&mut self, protocol: Protocol, cx: &mut Context<Self>) {
        let port = self.input(Field::Port).clone();
        if port.read(cx).text().trim() == self.protocol.default_port().to_string() {
            port.update(cx, |p, cx| p.set_text(protocol.default_port().to_string(), cx));
        }
        self.protocol = protocol;
        self.protocol_open = false;
        cx.notify();
    }

    fn labelled(&self, label: &'static str, field: Field, tip: Option<&'static str>, window: &Window, cx: &gpui::App) -> gpui::Div {
        let label_row = div().flex().items_center().gap(px(4.0)).child(dialog::label(label, cx)).when_some(tip, |d, tip| {
            d.child(ui::info_tip(SharedString::from(format!("env-tip-{label}")), tip, cx))
        });
        div().flex().flex_col().gap(px(4.0)).flex_1().min_w_0().child(label_row).child(dialog::text_field(self.input(field), window, cx))
    }
}

impl gpui::Render for EnvironmentDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let form = self.form(cx);
        let typed = !self.secret.read(cx).password(cx).is_empty();
        let problem = problem(&form, self.has_secret, typed);
        let row = || div().flex().gap(px(10.0)).w_full();
        let kinds: Vec<(SharedString, bool)> =
            EnvironmentKind::ALL.iter().map(|k| (SharedString::from(k.name()), *k == self.kind)).collect();
        let kind = dialog::select(
            "env-kind",
            self.kind.name(),
            self.kind_open,
            kinds,
            cx.listener(|this, open: &bool, _, cx| {
                this.kind_open = *open;
                cx.notify();
            }),
            cx.listener(|this, i: &usize, _, cx| {
                this.kind = EnvironmentKind::ALL[*i];
                this.kind_open = false;
                cx.notify();
            }),
            cx,
        );
        let protocols: Vec<(SharedString, bool)> =
            Protocol::ALL.iter().map(|p| (SharedString::from(p.name()), *p == self.protocol)).collect();
        let protocol = dialog::select(
            "env-protocol",
            self.protocol.name(),
            self.protocol_open,
            protocols,
            cx.listener(|this, open: &bool, _, cx| {
                this.protocol_open = *open;
                cx.notify();
            }),
            cx.listener(|this, i: &usize, _, cx| this.choose_protocol(Protocol::ALL[*i], cx)),
            cx,
        );
        let sftp = self.protocol == Protocol::Sftp;
        let use_key = self.use_key && sftp;
        let secret_label = match (use_key, self.has_secret) {
            (false, false) => "Password",
            (false, true) => "Password (empty: keep the saved one)",
            (true, false) => "Key passphrase (if it has one)",
            (true, true) => "Key passphrase (empty: keep the saved one)",
        };
        let body = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(
                row().child(self.labelled("Name", Field::Name, None, window, cx)).child(
                    div().flex().flex_col().gap(px(4.0)).w(px(170.0)).child(dialog::label("Kind", cx)).child(kind),
                ),
            )
            .child(
                row()
                    .child(div().flex().flex_col().gap(px(4.0)).w(px(200.0)).child(dialog::label("Protocol", cx)).child(protocol))
                    .child(self.labelled("Host", Field::Host, None, window, cx))
                    .child(div().w(px(90.0)).child(self.labelled("Port", Field::Port, None, window, cx))),
            )
            .when(self.protocol == Protocol::Ftp, |d| {
                d.child(dialog::hint(
                    "Plain FTP sends the password and every file unencrypted. Use SFTP or FTPS if the server has either - DNN Manager asks once more before it connects.",
                    cx,
                ))
            })
            .when(self.protocol == Protocol::Ftps, |d| {
                d.child(dialog::hint(
                    "The server's certificate is checked against the ones this computer trusts; a self-signed one is refused.",
                    cx,
                ))
            })
            .child(self.labelled("User", Field::User, None, window, cx))
            .when(sftp, |d| d.child(
                row()
                    .child(dialog::radio("env-auth-password", !self.use_key, "Password", cx).on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            this.use_key = false;
                            cx.notify();
                        },
                    )))
                    .child(dialog::radio("env-auth-key", self.use_key, "Private key file", cx).on_click(cx.listener(
                        |this, _: &ClickEvent, _, cx| {
                            this.use_key = true;
                            cx.notify();
                        },
                    ))),
            ))
            .when(use_key, |d| d.child(self.labelled("Key file", Field::KeyFile, Some("An OpenSSH private key (id_ed25519, id_rsa)."), window, cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(dialog::label(secret_label, cx))
                    .child(self.secret.clone()),
            )
            .child(self.labelled(
                "Site folder on the server",
                Field::RemoteRoot,
                Some("Where the whole project goes when there are no mappings."),
                window,
                cx,
            ))
            .child(self.labelled(
                "Mappings (optional)",
                Field::Mappings,
                Some("Only these parts of the project, each to its own server folder: project folder => /server/folder, ; between them. Empty: the whole project to the site folder."),
                window,
                cx,
            ))
            .child(self.labelled(
                "Also ignore",
                Field::Ignore,
                Some("Never compared or sent, on top of the defaults (.git/, node_modules/, App_Data/, logs, caches, *.tmp …). Rules as .gitignore writes them, ; between them."),
                window,
                cx,
            ))
            .child(self.labelled(
                "Protected",
                Field::Protected,
                Some("Replaced or deleted on the server only after a question naming them. On Production web.config, *.config, bin/, App_Data/ and Portals/ are protected too."),
                window,
                cx,
            ))
            .child(
                dialog::check("env-backup", self.backup, "Back up the server's copy before replacing or deleting it", cx)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                        this.backup = !this.backup;
                        cx.notify();
                    })),
            )
            .children(dialog::field_error(problem.clone().map(SharedString::from), cx));
        let ok = problem.is_none();
        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", if self.is_new { "Add" } else { "Save" }, ui::ButtonKind::Primary, ok, cx)
                    .min_w(px(80.0))
                    .tip("Save the environment (Enter)")
                    .when(ok, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))),
            )
            .child(
                ui::button("dialog-cancel", "Cancel", ui::ButtonKind::Secondary, true, cx)
                    .min_w(px(80.0))
                    .tip("Cancel (Esc)")
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DialogEvent::Cancelled))),
            );
        let title = if self.is_new { "Add environment" } else { "Edit environment" };
        div()
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "enter" && !this.kind_open && !this.protocol_open {
                    this.submit(cx);
                }
            }))
            .child(dialog::frame(title, 560.0, body, buttons, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> Form {
        Form {
            protocol: Protocol::Sftp,
            name: "Production".into(),
            host: "shop.example.com".into(),
            port: "22".into(),
            user: "deploy".into(),
            key_file: None,
            remote_root: "/var/www/shop".into(),
            mappings: String::new(),
            ignore: String::new(),
            protected: String::new(),
        }
    }

    #[test]
    fn a_complete_form_has_no_problem() {
        assert_eq!(problem(&form(), false, true), None);
        assert_eq!(problem(&form(), true, false), None, "the saved password stays");
        assert_eq!(problem(&form(), false, false).as_deref(), Some("Enter the password."));
    }

    #[test]
    fn each_field_is_checked() {
        type Case = (fn(&mut Form), &'static str);
        let cases: [Case; 7] = [
            (|f| f.name.clear(), "Name the environment."),
            (|f| f.host = "shop example".into(), "Enter the server's host name"),
            (|f| f.port = "70000".into(), "The port is a number"),
            (|f| f.user = " ".into(), "Enter the user"),
            (|f| f.remote_root = "var/www".into(), "The site's folder on the server is a full path"),
            (|f| f.mappings = "DesktopModules/Shop -> /srv".into(), "write it as a project folder => a server folder"),
            (|f| f.ignore = "../secrets".into(), "can't climb out of the project"),
        ];
        for (change, expected) in cases {
            let mut f = form();
            change(&mut f);
            let p = problem(&f, true, false).unwrap();
            assert!(p.contains(expected), "{p}");
        }
        let mut key = form();
        key.key_file = Some(String::new());
        assert_eq!(problem(&key, false, false).as_deref(), Some("Enter the path of the private key file."));
        key.key_file = Some("/home/me/.ssh/id_ed25519".into());
        assert_eq!(problem(&key, false, false), None, "a key needs no password");
        key.protocol = Protocol::Ftps;
        assert!(problem(&key, false, false).unwrap().contains("SFTP only"));
    }

    #[test]
    fn mappings_and_lists_are_read_from_one_line() {
        assert_eq!(
            parse_mappings("DesktopModules\\Shop => /srv/shop/ ; Portals/_default/Skins/Mine=>/skins").unwrap(),
            vec![
                Mapping { local: "DesktopModules/Shop".into(), remote: "/srv/shop".into() },
                Mapping { local: "Portals/_default/Skins/Mine".into(), remote: "/skins".into() },
            ]
        );
        assert!(parse_mappings("a => relative").unwrap_err().contains("full path"));
        assert!(parse_mappings("../up => /x").unwrap_err().contains("leaves the project"));
        assert_eq!(items(" *.map;; Temp/ ;"), ["*.map", "Temp/"]);
        let mut f = form();
        f.ignore = "*.map; Temp/".into();
        f.key_file = Some("/k".into());
        let base = Environment { host_key: Some("SHA256:x".into()), ..crate::source_control_environment::tests::blank() };
        let env = environment(&f, EnvironmentKind::Production, true, &base);
        assert_eq!(env.ignore, ["*.map", "Temp/"]);
        assert_eq!(env.auth, Auth::KeyFile("/k".into()));
        assert_eq!(env.host_key.as_deref(), Some("SHA256:x"), "the trusted key is kept");
        assert!(new_id("My Shop").starts_with("my-shop-"));
    }

    pub(crate) fn blank() -> Environment {
        Environment {
            id: "x".into(),
            project: "shop".into(),
            name: String::new(),
            kind: EnvironmentKind::Development,
            protocol: dnnm_application::source_control::model::Protocol::Sftp,
            host: String::new(),
            port: 22,
            user: String::new(),
            auth: Auth::Password,
            remote_root: String::new(),
            mappings: Vec::new(),
            ignore: Vec::new(),
            protected: Vec::new(),
            backup_remote: true,
            host_key: None,
        }
    }
}
