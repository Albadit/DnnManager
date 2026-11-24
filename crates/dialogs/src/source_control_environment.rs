//! Add or edit a Source Control server, laid out as FileZilla's Site Manager - a label before each field: its name,
//! the protocol (FTP or SFTP), host and port, FTP's encryption (explicit TLS, or plain), the logon type (a password,
//! or for SFTP a key file), user and password, and the default remote directory (`/site/wwwroot`, Azure App Service's);
//! then - one line each, `;` between items - the mappings (`local => /remote`), the ignore rules and the protected
//! paths, and whether the server's files are backed up before they are replaced. A server isn't tied to a project:
//! every project can be compared with it.
//! Emits `DialogEvent::Done(EnvironmentEdit)`; the app saves it (`dnnm_wiring::source_control::save_environment`).
//!
//! The password (or the key's passphrase) is never shown: left empty, the saved one stays.

use dnnm_ui::px;

use gpui::{ClickEvent, Context, Entity, EventEmitter, Focusable, KeyDownEvent, SharedString, Subscription, Window, div, prelude::*};

use dnnm_application::source_control::model::{Auth, Environment, Mapping, Protocol};
use dnnm_application::source_control::paths::clean;
use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::{self as ui, Tip};
use dnnm_ui::theme::Theme;

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

/// Where a new server's site is: Azure App Service's folder.
pub const DEFAULT_REMOTE_DIRECTORY: &str = "/site/wwwroot";

/// The protocols as FileZilla offers them: FTP (with its encryption chosen apart) and SFTP.
const FAMILIES: [(&str, Protocol); 2] = [("FTP - File Transfer Protocol", Protocol::Ftps), ("SFTP - SSH File Transfer Protocol", Protocol::Sftp)];

/// FTP's encryption, as FileZilla words it.
const ENCRYPTIONS: [(&str, Protocol); 2] = [("Require explicit FTP over TLS", Protocol::Ftps), ("Only use plain FTP (insecure)", Protocol::Ftp)];

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
        return Some("Name the server.".into());
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
        return Some("The default remote directory is a full path, starting with / (e.g. /site/wwwroot).".into());
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

/// The environment the form describes, keeping what the form doesn't hold (its id, its kind, the trusted key).
pub fn environment(form: &Form, backup: bool, base: &Environment) -> Environment {
    Environment {
        name: form.name.trim().into(),
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
    protocol: Protocol,
    protocol_open: bool,
    encryption_open: bool,
    logon_open: bool,
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
            (Field::Name, env.name.clone(), "Live server"),
            (Field::Host, env.host.clone(), "waws-prod-xx-000.ftp.azurewebsites.windows.net"),
            (Field::Port, env.port.to_string(), "21"),
            (Field::User, env.user.clone(), "app\\$app"),
            (Field::KeyFile, key_file, "~/.ssh/id_ed25519"),
            (Field::RemoteRoot, env.remote_root.clone(), DEFAULT_REMOTE_DIRECTORY),
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
            use_key: matches!(env.auth, Auth::KeyFile(_)),
            protocol: env.protocol,
            protocol_open: false,
            encryption_open: false,
            logon_open: false,
            backup: env.backup_remote,
            base: env,
            is_new,
            has_secret,
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
        let env = environment(&form, self.backup, &self.base);
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
        self.encryption_open = false;
        cx.notify();
    }

    /// One line of the form: its label (with a tip when given) in the left column, `content` filling the rest.
    fn line(label: &'static str, tip: Option<&'static str>, content: impl IntoElement, cx: &gpui::App) -> gpui::Div {
        let label_cell = div()
            .w(px(LABEL_WIDTH))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(dialog::label(label, cx))
            .when_some(tip, |d, tip| d.child(ui::info_tip(SharedString::from(format!("env-tip-{label}")), tip, cx)));
        // A fixed width, not `flex_1`: a text field has no width of its own and stayed a sliver in a column that only
        // grew into the line (seen in the window). A select fills it too; the check box goes in through `fit`.
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(label_cell)
            .child(div().w(px(VALUE_WIDTH)).flex_none().flex().flex_col().child(content))
    }

    /// A control that keeps its own width on a line (the check box) instead of stretching across it.
    fn fit(control: impl IntoElement) -> gpui::Div {
        div().flex().child(control)
    }

    fn field(&self, field: Field, window: &Window, cx: &gpui::App) -> gpui::Div {
        dialog::text_field(self.input(field), window, cx)
    }
}

/// The form's label column - as wide as its longest label.
const LABEL_WIDTH: f32 = 160.0;
/// The dialog's width.
const DIALOG_WIDTH: f32 = 620.0;
/// The value column: the dialog less its border (2), padding (2 × 20), the label column and the gap (10).
const VALUE_WIDTH: f32 = DIALOG_WIDTH - 2.0 - 40.0 - LABEL_WIDTH - 10.0;
/// Port's label and box beside the host.
const PORT_LABEL_WIDTH: f32 = 34.0;
const PORT_WIDTH: f32 = 70.0;

impl gpui::Render for EnvironmentDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let form = self.form(cx);
        let typed = !self.secret.read(cx).password(cx).is_empty();
        let problem = problem(&form, self.has_secret, typed);
        let sftp = self.protocol == Protocol::Sftp;
        let use_key = self.use_key && sftp;

        let family = if sftp { 1 } else { 0 };
        let protocol = dialog::select(
            "env-protocol",
            FAMILIES[family].0,
            self.protocol_open,
            FAMILIES.iter().enumerate().map(|(i, (name, _))| (SharedString::from(*name), i == family)).collect(),
            cx.listener(|this, open: &bool, _, cx| {
                this.protocol_open = *open;
                cx.notify();
            }),
            cx.listener(|this, i: &usize, _, cx| {
                // FTP keeps its encryption; SFTP is SFTP.
                let next = match (FAMILIES[*i].1, this.protocol) {
                    (Protocol::Ftps, Protocol::Ftp) => Protocol::Ftp,
                    (chosen, _) => chosen,
                };
                this.choose_protocol(next, cx);
            }),
            cx,
        );
        let encryption_at = if self.protocol == Protocol::Ftp { 1 } else { 0 };
        let encryption = dialog::select(
            "env-encryption",
            ENCRYPTIONS[encryption_at].0,
            self.encryption_open,
            ENCRYPTIONS.iter().enumerate().map(|(i, (name, _))| (SharedString::from(*name), i == encryption_at)).collect(),
            cx.listener(|this, open: &bool, _, cx| {
                this.encryption_open = *open;
                cx.notify();
            }),
            cx.listener(|this, i: &usize, _, cx| this.choose_protocol(ENCRYPTIONS[*i].1, cx)),
            cx,
        );
        // FileZilla's Logon Type: a password, or - over SFTP - a key file.
        let logons: &[&str] = if sftp { &["Normal", "Key file"] } else { &["Normal"] };
        let logon = dialog::select(
            "env-logon",
            if use_key { "Key file" } else { "Normal" },
            self.logon_open,
            logons.iter().enumerate().map(|(i, name)| (SharedString::from(*name), (i == 1) == use_key)).collect(),
            cx.listener(|this, open: &bool, _, cx| {
                this.logon_open = *open;
                cx.notify();
            }),
            cx.listener(|this, i: &usize, _, cx| {
                this.use_key = *i == 1;
                this.logon_open = false;
                cx.notify();
            }),
            cx,
        );
        let secret_label = if use_key { "Key passphrase" } else { "Password" };
        let secret_tip = match (use_key, self.has_secret) {
            (false, true) => Some("Empty: the saved password stays."),
            (true, false) => Some("Only when the key has one."),
            (true, true) => Some("Empty: the saved passphrase stays."),
            (false, false) => None,
        };
        let host_and_port = div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .w(px(VALUE_WIDTH - PORT_LABEL_WIDTH - PORT_WIDTH - 20.0))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .child(self.field(Field::Host, window, cx)),
            )
            .child(div().w(px(PORT_LABEL_WIDTH)).flex_none().child(dialog::label("Port", cx)))
            .child(div().w(px(PORT_WIDTH)).flex_none().flex().flex_col().child(self.field(Field::Port, window, cx)));
        let hint = match self.protocol {
            Protocol::Ftp => Some(
                "Plain FTP sends the password and every file unencrypted. Use SFTP or TLS if the server has either - DNN Manager asks once more before it connects.",
            ),
            Protocol::Ftps => Some("The server's certificate is checked against the ones this computer trusts; a self-signed one is refused."),
            Protocol::Sftp => None,
        };
        let divider = || div().w_full().h(px(1.0)).bg(Theme::get(cx).colors.card_border);

        // The dialog's width, whatever is shown: the body's scroll area doesn't stretch it.
        let body = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(Self::line("Name", None, self.field(Field::Name, window, cx), cx))
            .child(divider())
            .child(Self::line("Protocol", None, protocol, cx))
            .child(Self::line("Host", None, host_and_port, cx))
            .when(!sftp, |d| d.child(Self::line("Encryption", None, encryption, cx)))
            .when_some(hint, |d, hint| d.child(Self::line("", None, dialog::hint(hint, cx), cx)))
            .child(divider())
            .child(Self::line("Logon Type", None, logon, cx))
            .child(Self::line("User", None, self.field(Field::User, window, cx), cx))
            .when(use_key, |d| {
                d.child(Self::line("Key file", Some("An OpenSSH private key (id_ed25519, id_rsa)."), self.field(Field::KeyFile, window, cx), cx))
            })
            .child(Self::line(secret_label, secret_tip, self.secret.clone(), cx))
            .child(divider())
            .child(Self::line(
                "Default remote directory",
                Some("Where the whole project goes when there are no mappings - /site/wwwroot on Azure App Service."),
                self.field(Field::RemoteRoot, window, cx),
                cx,
            ))
            .child(Self::line(
                "Mappings",
                Some("Optional. Only these parts of the project, each to its own server folder: project folder => /server/folder, ; between them. Empty: the whole project to the default remote directory."),
                self.field(Field::Mappings, window, cx),
                cx,
            ))
            .child(Self::line(
                "Also ignore",
                Some("Never compared or sent, on top of the defaults (.git/, node_modules/, App_Data/, logs, caches, *.tmp …). Rules as .gitignore writes them, ; between them."),
                self.field(Field::Ignore, window, cx),
                cx,
            ))
            .child(Self::line(
                "Protected",
                Some("Replaced or deleted on the server only after a question naming them - web.config, *.config, bin/, App_Data/ and Portals/ always are."),
                self.field(Field::Protected, window, cx),
                cx,
            ))
            .child(Self::line(
                "",
                None,
                Self::fit(
                    dialog::check("env-backup", self.backup, "Back up the server's copy before replacing or deleting it", cx)
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.backup = !this.backup;
                            cx.notify();
                        })),
                ),
                cx,
            ))
            .children(dialog::field_error(problem.clone().map(SharedString::from), cx));
        let ok = problem.is_none();
        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", if self.is_new { "Add" } else { "Save" }, ui::ButtonKind::Primary, ok, cx)
                    .min_w(px(80.0))
                    .tip("Save the server (Enter)")
                    .when(ok, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.submit(cx)))),
            )
            .child(
                ui::button("dialog-cancel", "Cancel", ui::ButtonKind::Secondary, true, cx)
                    .min_w(px(80.0))
                    .tip("Cancel (Esc)")
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DialogEvent::Cancelled))),
            );
        let title = if self.is_new { "Add server" } else { "Edit server" };
        div()
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "enter" && !this.protocol_open && !this.encryption_open && !this.logon_open {
                    this.submit(cx);
                }
            }))
            .child(dialog::frame(title, DIALOG_WIDTH, body, buttons, cx))
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
            (|f| f.name.clear(), "Name the server."),
            (|f| f.host = "shop example".into(), "Enter the server's host name"),
            (|f| f.port = "70000".into(), "The port is a number"),
            (|f| f.user = " ".into(), "Enter the user"),
            (|f| f.remote_root = "site/wwwroot".into(), "The default remote directory is a full path"),
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
        let env = environment(&f, true, &base);
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
            kind: dnnm_application::source_control::model::EnvironmentKind::Production,
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
