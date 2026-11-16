//! Change database connection (`Controls/DatabaseConnectionDialog.xaml(.cs)`): where a site's web.config points
//! SiteSqlServer - the connection type, server, sign-in and database, the same fields as New project's Database card.
//! Nothing in a database is moved or copied - only the connection changes, so it can be tested here (not required)
//! before it is saved; saving what just failed asks for a second look. Emits `DialogEvent::Done(DatabaseConnection)`;
//! the app runs `dnnm_wiring::change_database_connection` with it.
//!
//! The fields' rules are [`ConnectionForm`], plain data the view reads its text fields into.

use std::sync::Arc;

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};

use dnnm_application::use_cases::database_connection::{
    ConnectionDefaults, DatabaseCheckReport, DatabaseConnection, DatabaseKind, SqlAuthentication,
};
use dnnm_ui::dialog::{self, DialogEvent};
use dnnm_ui::text_input::{TextChanged, TextInput};
use dnnm_ui::theme::Theme;
use dnnm_ui::{self as ui, Tooltip};

use crate::database_check_list::{CheckListState, database_check_list};
use crate::host_password::{PasswordField, PasswordFieldChanged};

/// Runs Test connection for the dialog - blocking, so the dialog calls it on the background executor:
/// `(connection, the Windows login the site signs in as)`. The app passes `dnnm_wiring`'s `test_connection`.
pub type ConnectionTester = Arc<dyn Fn(&DatabaseConnection, Option<&str>) -> DatabaseCheckReport + Send + Sync>;

/// The intro: what is changed, and that nothing is moved.
pub fn intro(site: &str) -> String {
    format!(
        "Where '{site}' finds its database - the SiteSqlServer connection string in its web.config. The site is \
         restarted after saving. Nothing in the databases is moved or copied: point it at a database that holds this \
         site, or restore a backup there first."
    )
}

/// What the fields say - the dialog's state without its controls.
#[derive(Clone, PartialEq, Eq)]
pub struct ConnectionForm {
    pub kind: DatabaseKind,
    pub server: String,
    /// The database box: a database name, or LocalDB's file in App_Data.
    pub name: String,
    pub windows_authentication: bool,
    pub user: String,
    pub password: String,
    // The box is shared: each meaning is kept while the other is shown, so switching back and forth loses nothing.
    database_name: String,
    local_db_file: String,
    // LocalDB isn't asked for its instance; a site already on another one keeps it.
    local_db_server: String,
    // The SQL Server fields are filled in the first time SQL Server is chosen - from the site, else the settings.
    sql_server_filled: bool,
}

impl ConnectionForm {
    /// A site without a connection starts on the local container, with a database named like the site.
    pub fn new(site: &str, current: Option<&DatabaseConnection>, defaults: &ConnectionDefaults) -> Self {
        let kind = current.map_or(DatabaseKind::Container, |c| c.kind);
        let file = current.filter(|c| c.kind == DatabaseKind::LocalDbFile);
        let database_name = match current {
            Some(c) if c.kind != DatabaseKind::LocalDbFile => c.database.clone(),
            _ => site.to_string(),
        };
        let local_db_file = file
            .map(|c| c.database.clone())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| DatabaseConnection::LOCAL_DB_FILE_NAME.to_string());
        let local_db_server = file
            .map(|c| c.server.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| DatabaseConnection::LOCAL_DB_SERVER.to_string());
        let mut form = Self {
            kind,
            server: String::new(),
            name: if kind == DatabaseKind::LocalDbFile { local_db_file.clone() } else { database_name.clone() },
            windows_authentication: true,
            user: String::new(),
            password: String::new(),
            database_name,
            local_db_file,
            local_db_server,
            sql_server_filled: false,
        };
        if kind == DatabaseKind::SqlServer {
            form.fill_sql_server(current, defaults);
        }
        form
    }

    /// The SQL Server fields: the site's own when it is on SQL Server, otherwise Settings → Database server's when
    /// that is a SQL Server (`FillSqlServer`).
    fn fill_sql_server(&mut self, current: Option<&DatabaseConnection>, defaults: &ConnectionDefaults) {
        self.sql_server_filled = true;
        if let Some(c) = current.filter(|c| c.kind == DatabaseKind::SqlServer) {
            self.server = c.server.clone();
            self.windows_authentication = c.authentication == SqlAuthentication::Windows;
            self.user = c.user.clone();
            self.password = c.password.clone();
        } else {
            let saved = defaults.server_is_sql_server();
            self.server =
                if saved && !defaults.server.is_empty() { defaults.server.clone() } else { r".\SQLEXPRESS".into() };
            self.windows_authentication = !defaults.server_uses_sql_authentication;
            self.user = if saved { defaults.server_user.clone() } else { String::new() };
            self.password =
                if defaults.server_uses_sql_authentication { defaults.server_password.clone() } else { String::new() };
        }
    }

    /// Another connection type (`DbType_Checked`): the database box changes meaning between a database name and
    /// LocalDB's file.
    pub fn switch_to(
        &mut self,
        kind: DatabaseKind,
        current: Option<&DatabaseConnection>,
        defaults: &ConnectionDefaults,
    ) {
        if kind == self.kind {
            return;
        }
        if self.kind == DatabaseKind::LocalDbFile {
            self.local_db_file = self.name.trim().to_string();
            self.name = self.database_name.clone();
        } else if kind == DatabaseKind::LocalDbFile {
            self.database_name = self.name.trim().to_string();
            self.name = self.local_db_file.clone();
        }
        if kind == DatabaseKind::SqlServer && !self.sql_server_filled {
            self.fill_sql_server(current, defaults);
        }
        self.kind = kind;
    }

    /// SQL Server with a SQL Server login: the username and password fields show.
    pub fn sql_login(&self) -> bool {
        self.kind == DatabaseKind::SqlServer && !self.windows_authentication
    }

    /// What's wrong with the fields, or None when they make a connection (`FieldProblem`).
    pub fn problem(&self) -> Option<String> {
        let name = self.name.trim();
        if self.kind == DatabaseKind::LocalDbFile {
            if name.is_empty() {
                return Some("Enter the database file's name.".into());
            }
            return (name.chars().any(invalid_file_name_char) || !name.to_lowercase().ends_with(".mdf")).then(|| {
                format!(
                    "Enter the name of a .mdf file in the site's App_Data folder, e.g. {}.",
                    DatabaseConnection::LOCAL_DB_FILE_NAME
                )
            });
        }
        if self.kind == DatabaseKind::SqlServer && self.server.trim().is_empty() {
            return Some("Enter the server.".into());
        }
        if name.is_empty() {
            return Some("Enter the database name.".into());
        }
        if name.encode_utf16().count() > 128 || name.contains(']') {
            return Some("The database name can have at most 128 characters, and no ']'.".into());
        }
        if self.sql_login() && self.user.trim().is_empty() {
            return Some("Enter the username.".into());
        }
        if self.sql_login() && self.password.is_empty() {
            return Some("Enter the password.".into());
        }
        None
    }

    /// The connection the fields describe - the container's server and login from the settings (`ChosenConnection`).
    pub fn connection(&self, defaults: &ConnectionDefaults) -> DatabaseConnection {
        let database = self.name.trim();
        match self.kind {
            DatabaseKind::Container => defaults.container_connection(database),
            DatabaseKind::LocalDbFile => DatabaseConnection::new(
                DatabaseKind::LocalDbFile,
                &self.local_db_server,
                database,
                SqlAuthentication::Windows,
            ),
            DatabaseKind::SqlServer if self.sql_login() => {
                DatabaseConnection::new(DatabaseKind::SqlServer, self.server.trim(), database, SqlAuthentication::Sql)
                    .with_login(self.user.trim(), &self.password)
            }
            DatabaseKind::SqlServer => DatabaseConnection::new(
                DatabaseKind::SqlServer,
                self.server.trim(),
                database,
                SqlAuthentication::Windows,
            ),
        }
    }

    /// The database box's label.
    pub fn name_label(&self) -> &'static str {
        if self.kind == DatabaseKind::LocalDbFile { "Database file in App_Data" } else { "Database name" }
    }

    /// The hint under the fields: where the database is and how the site signs in.
    pub fn hint(&self, site_login: &str, defaults: &ConnectionDefaults) -> String {
        let login = if site_login.is_empty() {
            "its app pool's identity".to_string()
        } else {
            format!("its app pool's identity ({site_login})")
        };
        match self.kind {
            DatabaseKind::Container => format!(
                "On the local SQL container at {}, signed in to as '{}' - as Settings → Database server has it.",
                defaults.container_server, defaults.container_user
            ),
            DatabaseKind::LocalDbFile => {
                let file = self.name.trim();
                let file = if file.is_empty() { DatabaseConnection::LOCAL_DB_FILE_NAME } else { file };
                format!(
                    "The site's own App_Data\\{file}, attached by LocalDB ({}) - the site signs in as {login}.",
                    self.local_db_server
                )
            }
            DatabaseKind::SqlServer if self.sql_login() => {
                "The site signs in with this SQL Server login - its password is written to web.config.".into()
            }
            DatabaseKind::SqlServer => format!("The site signs in as {login}."),
        }
    }
}

/// `Path.GetInvalidFileNameChars` on Windows.
fn invalid_file_name_char(c: char) -> bool {
    matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?' | '\\' | '/') || (c as u32) < 32
}

/// What the dialog shows below the fields, from the form and the last test (`UpdateState`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormState {
    pub problem: Option<String>,
    pub can_test: bool,
    pub can_save: bool,
    pub check_list: CheckListState,
}

/// The state for `form` given the last test - shown only while the fields still say what was tested.
pub fn form_state(
    form: &ConnectionForm,
    defaults: &ConnectionDefaults,
    current: Option<&DatabaseConnection>,
    testing: bool,
    tested: Option<&(DatabaseConnection, DatabaseCheckReport)>,
) -> FormState {
    let problem = form.problem();
    let connection = problem.is_none().then(|| form.connection(defaults));
    let tested_now = match (&connection, tested) {
        (Some(c), Some((t, report))) if c.same_as(t) => Some(report),
        _ => None,
    };
    let check_list = if testing {
        CheckListState::Testing
    } else {
        tested_now.map_or(CheckListState::Empty, |r| CheckListState::Report(r.clone()))
    };
    // Testing isn't required, but saving what just failed is worth a second look.
    let failed = tested_now.is_some_and(|r| !r.passed());
    FormState {
        problem: problem.or_else(|| failed.then(|| "The last test failed - save anyway?".into())),
        can_test: connection.is_some() && !testing,
        can_save: connection.as_ref().is_some_and(|c| current.is_none_or(|current| !c.same_as(current))),
        check_list,
    }
}

pub struct DatabaseConnectionDialog {
    site: String,
    current: Option<DatabaseConnection>,
    site_login: String,
    defaults: ConnectionDefaults,
    tester: ConnectionTester,
    form: ConnectionForm,
    server: Entity<TextInput>,
    name: Entity<TextInput>,
    user: Entity<TextInput>,
    password: Entity<PasswordField>,
    testing: bool,
    tested: Option<(DatabaseConnection, DatabaseCheckReport)>,
    _test: Option<Task<()>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent<DatabaseConnection>> for DatabaseConnectionDialog {}

impl DatabaseConnectionDialog {
    /// `current`: the site's connection now (`dnnm_wiring::change_database_connection::database_of`); `site_login`:
    /// its app pool's identity; `defaults`: `connection_defaults` of the settings.
    pub fn new(
        site: &str,
        current: Option<DatabaseConnection>,
        site_login: &str,
        defaults: ConnectionDefaults,
        tester: ConnectionTester,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let form = ConnectionForm::new(site, current.as_ref(), &defaults);
        let input = |text: &str, cx: &mut Context<Self>| {
            let input = cx.new(|cx| TextInput::new("", cx));
            input.update(cx, |i, cx| i.set_text(text.to_string(), cx));
            input
        };
        let (server, name, user) = (input(&form.server, cx), input(&form.name, cx), input(&form.user, cx));
        let password = cx.new(PasswordField::new);
        password.update(cx, |p, cx| p.set_password(&form.password, cx));
        let edited = |this: &mut Self, _: Entity<TextInput>, _: &TextChanged, cx: &mut Context<Self>| {
            this.read_fields(cx);
            cx.notify();
        };
        let subscriptions = vec![
            cx.subscribe(&server, edited),
            cx.subscribe(&name, edited),
            cx.subscribe(&user, edited),
            cx.subscribe(&password, |this, _, _: &PasswordFieldChanged, cx| {
                this.read_fields(cx);
                cx.notify();
            }),
        ];
        let first = if form.kind == DatabaseKind::SqlServer { &server } else { &name };
        window.focus(&first.focus_handle(cx));
        Self {
            site: site.to_string(),
            current,
            site_login: site_login.to_string(),
            defaults,
            tester,
            form,
            server,
            name,
            user,
            password,
            testing: false,
            tested: None,
            _test: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    fn read_fields(&mut self, cx: &App) {
        self.form.server = self.server.read(cx).text().to_string();
        self.form.name = self.name.read(cx).text().to_string();
        self.form.user = self.user.read(cx).text().to_string();
        self.form.password = self.password.read(cx).password(cx);
    }

    fn switch_to(&mut self, kind: DatabaseKind, cx: &mut Context<Self>) {
        self.read_fields(cx);
        self.form.switch_to(kind, self.current.as_ref(), &self.defaults);
        let form = self.form.clone();
        self.server.update(cx, |i, cx| i.set_text(form.server.clone(), cx));
        self.name.update(cx, |i, cx| i.set_text(form.name.clone(), cx));
        self.user.update(cx, |i, cx| i.set_text(form.user.clone(), cx));
        self.password.update(cx, |p, cx| p.set_password(&form.password, cx));
        cx.notify();
    }

    fn set_windows_authentication(&mut self, windows: bool, cx: &mut Context<Self>) {
        self.form.windows_authentication = windows;
        cx.notify();
    }

    fn state(&self) -> FormState {
        form_state(&self.form, &self.defaults, self.current.as_ref(), self.testing, self.tested.as_ref())
    }

    fn test(&mut self, cx: &mut Context<Self>) {
        if self.form.problem().is_some() || self.testing {
            return;
        }
        let connection = self.form.connection(&self.defaults);
        // With Windows authentication the site signs in as its app pool: that login is checked too.
        let login =
            (connection.uses_windows_authentication() && !self.site_login.is_empty()).then(|| self.site_login.clone());
        self.testing = true;
        let tester = self.tester.clone();
        let tested = connection.clone();
        self._test = Some(cx.spawn(async move |this, cx| {
            let report = cx.background_executor().spawn(async move { tester(&tested, login.as_deref()) }).await;
            let _ = this.update(cx, |d, cx| {
                d.testing = false;
                d.tested = Some((connection, report));
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        if self.state().can_save {
            cx.emit(DialogEvent::Done(self.form.connection(&self.defaults)));
        }
    }
}

impl Focusable for DatabaseConnectionDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DatabaseConnectionDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx).clone();
        let state = self.state();
        let kind = self.form.kind;
        let sql_server = kind == DatabaseKind::SqlServer;

        let kind_radio = |id: &'static str, text: &'static str, of: DatabaseKind, cx: &mut Context<Self>| {
            dialog::radio(id, kind == of, text, cx)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.switch_to(of, cx)))
        };
        // A label 4 above its box.
        let labelled = |label: SharedString, control: AnyElement, cx: &App| {
            div().flex_1().min_w_0().flex().flex_col().gap(px(4.0)).child(dialog::label(label, cx)).child(control)
        };
        let name_field =
            labelled(self.form.name_label().into(), dialog::text_field(&self.name, window, cx).into_any_element(), cx);

        // The fields 10 apart (the radio buttons keep 4 above and under them), and the hint 4 under them.
        let windows = self.form.windows_authentication;
        let sql_login = self.form.sql_login();
        let fields = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            // Only SQL Server asks for its server: the container's and the LocalDB instance are fixed. The name box
            // sits on its own row when it is the only field.
            .child(
                div()
                    .flex()
                    .gap(px(12.0))
                    .when(sql_server, |d| {
                        d.child(labelled(
                            "Server".into(),
                            dialog::text_field(&self.server, window, cx).into_any_element(),
                            cx,
                        ))
                    })
                    .child(name_field),
            )
            .when(sql_server, |d| {
                d.child(
                    div()
                        .flex()
                        .gap(px(20.0))
                        .child(dialog::radio("db-windows-auth", windows, "Windows authentication", cx).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| this.set_windows_authentication(true, cx)),
                        ))
                        .child(dialog::radio("db-sql-auth", !windows, "SQL Server authentication", cx).on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| this.set_windows_authentication(false, cx)),
                        )),
                )
            })
            .when(sql_login, |d| {
                d.child(
                    div()
                        .flex()
                        .gap(px(12.0))
                        .child(labelled(
                            "Username".into(),
                            dialog::text_field(&self.user, window, cx).into_any_element(),
                            cx,
                        ))
                        .child(labelled("Password".into(), self.password.clone().into_any_element(), cx)),
                )
            });
        let fields = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(fields)
            .child(dialog::hint(self.form.hint(&self.site_login, &self.defaults), cx));

        // Test connection, what it found 11 under it (8 while it runs), then the problem 10 under that.
        let checks = match state.check_list {
            CheckListState::Empty => None,
            _ => Some(database_check_list(&state.check_list, cx)),
        };
        let list_gap = if matches!(state.check_list, CheckListState::Report(_)) { 11.0 } else { 8.0 };
        let test = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(list_gap))
            .child(
                div().flex().child(
                    ui::button(
                        "db-test",
                        if self.testing { "Testing…" } else { "Test connection" },
                        ui::ButtonKind::Secondary,
                        state.can_test,
                        cx,
                    )
                    .min_w(px(80.0))
                    .tooltip(Tooltip::text("Test connection"))
                    .when(state.can_test, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.test(cx)))),
                ),
            )
            .children(checks);
        let test = div()
            .w_full()
            .flex()
            .flex_col()
            // With nothing tested, the problem keeps the room of the empty list above it.
            .gap(px(if matches!(state.check_list, CheckListState::Empty) { 18.0 } else { 10.0 }))
            .child(test)
            .children(dialog::field_error(state.problem.clone().map(SharedString::from), cx));

        let body = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(div().w_full().child(intro(&self.site)))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
                            .child(div().w_full().child("Connection type"))
                            .child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .gap_x(px(20.0))
                                    .child(kind_radio(
                                        "db-container",
                                        "Local SQL container (Docker)",
                                        DatabaseKind::Container,
                                        cx,
                                    ))
                                    .child(kind_radio("db-sql-server", "SQL Server", DatabaseKind::SqlServer, cx))
                                    .child(kind_radio(
                                        "db-local-db",
                                        "SQL Server Express LocalDB (file)",
                                        DatabaseKind::LocalDbFile,
                                        cx,
                                    )),
                            ),
                    )
                    .child(fields),
            )
            .child(test);

        let buttons = div()
            .flex()
            .gap(px(8.0))
            .child(
                ui::button("dialog-ok", "Save", ui::ButtonKind::Primary, state.can_save, cx)
                    .min_w(px(80.0))
                    .tooltip(Tooltip::text("Save (Enter)"))
                    .when(state.can_save, |b| b.on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.save(cx)))),
            )
            .child(
                ui::button("dialog-cancel", "Cancel", ui::ButtonKind::Secondary, true, cx)
                    .min_w(px(80.0))
                    .tooltip(Tooltip::text("Cancel (Esc)"))
                    .on_click(cx.listener(|_, _: &ClickEvent, _, cx| cx.emit(DialogEvent::Cancelled))),
            );

        div()
            .track_focus(&self.focus)
            .text_size(theme.text.body)
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "enter" {
                    this.save(cx);
                }
            }))
            .child(dialog::frame("Change database connection", 520.0, body, buttons, cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dnnm_application::use_cases::database_connection::{CheckOutcome, DatabaseCheck};

    fn defaults() -> ConnectionDefaults {
        ConnectionDefaults {
            container_server: "localhost,1433".into(),
            container_user: "sa".into(),
            container_password: "Sa-Secret-1".into(),
            server_type: "container".into(),
            server: String::new(),
            ..Default::default()
        }
    }

    fn sql_server(user: &str, password: &str) -> DatabaseConnection {
        DatabaseConnection::new(DatabaseKind::SqlServer, "db01", "shop", SqlAuthentication::Sql)
            .with_login(user, password)
    }

    #[test]
    fn a_site_without_a_connection_starts_on_the_container_named_like_it() {
        let form = ConnectionForm::new("shop", None, &defaults());
        assert_eq!((form.kind, form.name.as_str()), (DatabaseKind::Container, "shop"));
        assert_eq!(form.connection(&defaults()), defaults().container_connection("shop"));
        assert_eq!(
            form.hint("", &defaults()),
            "On the local SQL container at localhost,1433, signed in to as 'sa' - as Settings → Database server has it."
        );
        let state = form_state(&form, &defaults(), None, false, None);
        assert!(state.can_save && state.can_test && state.problem.is_none());
    }

    #[test]
    fn switching_types_keeps_each_meaning_of_the_database_box() {
        let mut form = ConnectionForm::new("shop", None, &defaults());
        form.name = "shop2 ".into();
        form.switch_to(DatabaseKind::LocalDbFile, None, &defaults());
        assert_eq!((form.name.as_str(), form.name_label()), ("Database.mdf", "Database file in App_Data"));
        assert_eq!(
            form.hint(r"IIS APPPOOL\shop", &defaults()),
            r"The site's own App_Data\Database.mdf, attached by LocalDB ((LocalDB)\MSSQLLocalDB) - the site signs in as its app pool's identity (IIS APPPOOL\shop)."
        );
        form.name = "Site.mdf".into();
        form.switch_to(DatabaseKind::SqlServer, None, &defaults());
        assert_eq!((form.name.as_str(), form.server.as_str()), ("shop2", r".\SQLEXPRESS"));
        assert!(form.windows_authentication && form.user.is_empty());
        assert_eq!(form.hint("", &defaults()), "The site signs in as its app pool's identity.");
        form.switch_to(DatabaseKind::LocalDbFile, None, &defaults());
        assert_eq!(form.name, "Site.mdf");
    }

    #[test]
    fn sql_server_is_filled_from_the_site_else_from_the_settings() {
        let current = sql_server("dnn", "pw");
        let form = ConnectionForm::new("shop", Some(&current), &defaults());
        assert_eq!((form.server.as_str(), form.user.as_str(), form.password.as_str()), ("db01", "dnn", "pw"));
        assert!(form.sql_login());
        assert_eq!(
            form.hint("", &defaults()),
            "The site signs in with this SQL Server login - its password is written to web.config."
        );

        let settings = ConnectionDefaults {
            server_type: "sqlServer".into(),
            server: "sql02".into(),
            server_uses_sql_authentication: true,
            server_user: "manager".into(),
            server_password: "from-credentials".into(),
            ..defaults()
        };
        let mut form = ConnectionForm::new("shop", None, &settings);
        form.switch_to(DatabaseKind::SqlServer, None, &settings);
        assert_eq!((form.server.as_str(), form.user.as_str()), ("sql02", "manager"));
        assert_eq!(form.password, "from-credentials");
        assert!(!form.windows_authentication);
    }

    #[test]
    fn problems_are_worded_as_the_wpf_dialog_words_them() {
        let mut form = ConnectionForm::new("shop", Some(&sql_server("u", "p")), &defaults());
        let problem = |f: &ConnectionForm| f.problem();
        form.server = " ".into();
        assert_eq!(problem(&form).as_deref(), Some("Enter the server."));
        form.server = "db".into();
        form.name = "".into();
        assert_eq!(problem(&form).as_deref(), Some("Enter the database name."));
        form.name = "a]b".into();
        assert_eq!(problem(&form).as_deref(), Some("The database name can have at most 128 characters, and no ']'."));
        form.name = "x".repeat(129);
        assert!(problem(&form).is_some());
        form.name = "shop".into();
        form.user = " ".into();
        assert_eq!(problem(&form).as_deref(), Some("Enter the username."));
        form.user = "u".into();
        form.password = "".into();
        assert_eq!(problem(&form).as_deref(), Some("Enter the password."));
        form.windows_authentication = true;
        assert_eq!(problem(&form), None);

        form.switch_to(DatabaseKind::LocalDbFile, None, &defaults());
        form.name = "".into();
        assert_eq!(problem(&form).as_deref(), Some("Enter the database file's name."));
        for bad in ["Site.ldf", "a:b.mdf", "x\\y.mdf"] {
            form.name = bad.into();
            assert_eq!(
                problem(&form).as_deref(),
                Some("Enter the name of a .mdf file in the site's App_Data folder, e.g. Database.mdf."),
                "{bad}"
            );
        }
        form.name = "Site.MDF".into();
        assert_eq!(problem(&form), None);
    }

    #[test]
    fn the_same_connection_cant_be_saved_and_a_failed_test_asks() {
        let current = sql_server("u", "p");
        let form = ConnectionForm::new("shop", Some(&current), &defaults());
        assert!(!form_state(&form, &defaults(), Some(&current), false, None).can_save);
        let mut other = form.clone();
        other.name = "Shop2".into();
        let chosen = other.connection(&defaults());
        assert_eq!(chosen, DatabaseConnection { database: "Shop2".into(), ..current.clone() });

        let failed = DatabaseCheckReport {
            checks: vec![DatabaseCheck {
                name: "Signed in".into(),
                outcome: CheckOutcome::Failed,
                detail: "no".into(),
            }],
        };
        let tested = (chosen.clone(), failed.clone());
        let state = form_state(&other, &defaults(), Some(&current), false, Some(&tested));
        assert_eq!(state.problem.as_deref(), Some("The last test failed - save anyway?"));
        assert!(state.can_save && state.check_list == CheckListState::Report(failed));
        // Another value hides the test's result; a test running shows that.
        let mut changed = other.clone();
        changed.name = "Shop3".into();
        let state = form_state(&changed, &defaults(), Some(&current), false, Some(&tested));
        assert_eq!((state.problem, state.check_list), (None, CheckListState::Empty));
        let state = form_state(&other, &defaults(), Some(&current), true, Some(&tested));
        assert!(!state.can_test && state.check_list == CheckListState::Testing);
    }

    #[test]
    fn the_intro_says_nothing_is_moved() {
        assert!(intro("shop").starts_with("Where 'shop' finds its database - the SiteSqlServer connection string"));
        assert!(intro("shop").ends_with("or restore a backup there first."));
    }
}
