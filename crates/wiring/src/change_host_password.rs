//! Change host password joined to DNN and SQL Server (`ChangeHostPasswordUseCase`, `DnnAccountRules`,
//! `DnnInstaller.ChangeHostPasswordAsync` with `MembershipPasswords`), and the host accounts its dialog offers
//! (`DatabaseProvisioner.ListHostAccountsAsync`). Every function here blocks: the app runs it off the UI thread.

use std::path::Path;

use dnnm_application::operation::{CANCELLED, CancelToken, Operation};
use dnnm_application::settings::Settings;
use dnnm_application::use_cases::change_host_password::{self as use_case, HostPasswordRequest};
use dnnm_application::use_cases::database_connection::{DatabaseConnection, DatabaseKind};
use dnnm_dnn::installer::{DbVersion, HostPasswordOutcome, Installer, InstallerSql, PasswordHasher, VersionOrder};
use dnnm_infrastructure::web_config;
use dnnm_sql::client::{Conn, SqlError, block_on};
use dnnm_sql::sql_text::dnn_tables;

use crate::Services;
use crate::change_database_connection::{database_from, database_of, to_sql};

/// The button on Details → DNN → Host accounts, and its tooltip (`ProjectView.xaml`).
pub const CHANGE_HOST_PASSWORD_BUTTON: &str = "Change host password…";
pub const CHANGE_HOST_PASSWORD_TOOLTIP: &str = "Change host password";
/// The hint under the button: when it can be used, and when not (`ProjectView.ShowHosts`).
pub const HOST_PASSWORD_HINT: &str = "Sets a new password the way DNN stores it and restarts the site. The current password is never shown - DNN keeps only a hash of it.";
pub const HOST_PASSWORD_HINT_NOT_YET: &str =
    "Once DNN is installed and its web.config names its database, the host password can be changed here.";

/// The operation's title (`ProjectView.ChangePassword_Click`).
pub fn change_host_password_title(site: &str) -> String {
    format!("Change host password of '{site}'")
}

/// The toast when it is done.
pub fn change_host_password_done(site: &str, user: &str) -> String {
    format!("'{user}' signs in to {site} with the new password.")
}

/// Change host password: the app's entry point, run as an operation.
pub fn change_host_password(
    services: &Services,
    request: HostPasswordRequest,
    op: &mut Operation,
) -> Result<(), String> {
    let dnn = DnnHostPassword { settings: services.settings.clone() };
    use_case::change_host_password(services.iis.as_ref(), &dnn, &request, op)
}

/// The host accounts of the database `connection` (the site's web.config's, as the Details read it), oldest first -
/// what the dialog offers. Not for a LocalDB file: that file is the site's own while it runs.
pub fn host_accounts(connection: &web_config::DatabaseConnection, settings: &Settings) -> Result<Vec<String>, String> {
    let Some(database) = database_from(connection, settings).filter(|d| d.kind != DatabaseKind::LocalDbFile) else {
        return Err("can't be read - the LocalDB file is the site's own while it runs".into());
    };
    let accounts = dnnm_sql::provisioner::DatabaseProvisioner.list_host_accounts(&to_sql(&database), None)?;
    Ok(accounts.into_iter().map(|a| a.user_name).collect())
}

struct DnnHostPassword {
    settings: Settings,
}

impl use_case::HostPasswordChanger for DnnHostPassword {
    fn password_problem(&self, password: &str) -> Option<String> {
        dnnm_dnn::install::password_problem(password)
    }

    fn database_of(&self, _site_name: &str, directory: &Path) -> Option<DatabaseConnection> {
        database_of(&self.settings, directory)
    }

    fn change_host_password(
        &self,
        directory: &Path,
        database: &DatabaseConnection,
        user_name: &str,
        new_password: &str,
        cancel: &CancelToken,
    ) -> Result<(), String> {
        let database = web_config::DatabaseConnection {
            kind: if database.kind == DatabaseKind::LocalDbFile {
                web_config::DatabaseKind::LocalDbFile
            } else {
                web_config::DatabaseKind::SqlServer
            },
            server: database.server.clone(),
            database: database.database.clone(),
            windows_authentication: database.uses_windows_authentication(),
            user: database.user.clone(),
            password: database.password.clone(),
        };
        Installer::new(HostPasswordSql).change_host_password(
            directory,
            &database,
            user_name,
            new_password,
            &MembershipHasher,
            cancel,
        )
    }
}

/// DNN's membership hashes (`MembershipPasswords`), from `dnnm-sql`.
struct MembershipHasher;

impl PasswordHasher for MembershipHasher {
    fn is_supported(&self, algorithm: &str) -> bool {
        dnnm_sql::membership::is_supported(algorithm)
    }

    fn new_salt(&self) -> String {
        dnnm_sql::membership::new_salt()
    }

    fn hash(&self, password: &str, salt: &str, algorithm: &str) -> String {
        // The installer asks only for a supported algorithm, with a salt of `new_salt` - always Base64.
        dnnm_sql::membership::hash(password, salt, algorithm).expect("a supported algorithm and a Base64 salt")
    }
}

/// The installer's SQL for setting a host password. Only `set_host_password` is DNN Manager's here: the install and
/// upgrade steps of `InstallerSql` belong to the New project wiring, which this use case never reaches - each of them
/// says so instead of doing anything.
struct HostPasswordSql;

const NOT_HERE: &str = "Change host password doesn't run DNN's installer.";

impl InstallerSql for HostPasswordSql {
    fn raise_compatibility_level(
        &self,
        _: &Path,
        _: &web_config::DatabaseConnection,
        _: &CancelToken,
    ) -> Result<(), String> {
        Err(NOT_HERE.into())
    }

    fn version(
        &self,
        _: &Path,
        _: &web_config::DatabaseConnection,
        _: VersionOrder,
        _: &CancelToken,
    ) -> Result<DbVersion, String> {
        Err(NOT_HERE.into())
    }

    fn portal_alias_count(
        &self,
        _: &Path,
        _: &web_config::DatabaseConnection,
        _: &str,
        _: &CancelToken,
    ) -> Result<i64, String> {
        Err(NOT_HERE.into())
    }

    fn finish_host(
        &self,
        _: &Path,
        _: &web_config::DatabaseConnection,
        _: &str,
        _: &CancelToken,
    ) -> Result<i64, String> {
        Err(NOT_HERE.into())
    }

    fn set_host_password(
        &self,
        site: &Path,
        database: &web_config::DatabaseConnection,
        application_name: &str,
        user: &str,
        hash: &str,
        salt: &str,
        cancel: &CancelToken,
    ) -> Result<HostPasswordOutcome, String> {
        let values = [application_name, user, hash, salt].map(str::to_string);
        let work = move |conn: Conn| set_password(conn, values);
        let connection = sql_connection(database);
        let result = if connection.kind == dnnm_sql::DatabaseKind::LocalDbFile {
            dnnm_sql::localdb::with_database(site, &connection, Some(cancel), work)
        } else {
            let settings = dnnm_sql::connection_strings::for_app(&connection, None, 30, None);
            block_on(Some(cancel), async move { work(Conn::open(&settings).await?).await })
        };
        result.map_err(|e| if e.cancelled { CANCELLED.to_string() } else { e.message })
    }

    fn try_connect(&self, _: &web_config::DatabaseConnection) -> Result<(), String> {
        Err(NOT_HERE.into())
    }
}

fn sql_connection(database: &web_config::DatabaseConnection) -> dnnm_sql::DatabaseConnection {
    let (kind, authentication) = match (database.kind, database.windows_authentication) {
        (web_config::DatabaseKind::LocalDbFile, _) => {
            (dnnm_sql::DatabaseKind::LocalDbFile, dnnm_sql::SqlAuthentication::Windows)
        }
        (_, true) => (dnnm_sql::DatabaseKind::SqlServer, dnnm_sql::SqlAuthentication::Windows),
        (_, false) => (dnnm_sql::DatabaseKind::SqlServer, dnnm_sql::SqlAuthentication::Sql),
    };
    dnnm_sql::DatabaseConnection::new(kind, &database.server, &database.database, authentication)
        .with_login(&database.user, &database.password)
}

/// What `DnnInstaller.ChangeHostPasswordAsync` runs, in one batch so its transaction ends with it:
/// `aspnet_Membership_SetPassword` (hashed, format 1), `aspnet_Membership_UnlockUser` and `UpdatePassword = 0`. The
/// C# passes `DateTime.UtcNow` as `@CurrentTimeUtc`; this takes the server's `GETUTCDATE()` - the parameters here
/// are text only.
fn set_password_sql(qualifier: &str) -> String {
    format!(
        "SET XACT_ABORT ON; DECLARE @now datetime = GETUTCDATE(); DECLARE @r int; BEGIN TRANSACTION; \
         EXEC @r = dbo.aspnet_Membership_SetPassword @ApplicationName = @P1, @UserName = @P2, @NewPassword = @P3, \
         @PasswordSalt = @P4, @CurrentTimeUtc = @now, @PasswordFormat = 1; \
         IF @r <> 0 BEGIN ROLLBACK TRANSACTION; SELECT @r; RETURN; END; \
         EXEC dbo.aspnet_Membership_UnlockUser @ApplicationName = @P1, @UserName = @P2; \
         UPDATE {} SET UpdatePassword = 0 WHERE Username = @P2 AND IsSuperUser = 1; \
         COMMIT TRANSACTION; SELECT 0;",
        dnn_tables::name(qualifier, "Users")
    )
}

async fn set_password(mut conn: Conn, values: [String; 4]) -> Result<HostPasswordOutcome, SqlError> {
    let [application, user, hash, salt] = values;
    let qualifier = conn
        .scalar(dnn_tables::QUALIFIER_SQL, &["Version".into()])
        .await?
        .into_text()
        .filter(|q| dnn_tables::is_qualifier(q));
    let Some(q) = qualifier else { return Ok(HostPasswordOutcome::NoDnnTables) };
    let host = conn
        .scalar(
            &format!("SELECT COUNT(*) FROM {} WHERE Username = @P1 AND IsSuperUser = 1", dnn_tables::name(&q, "Users")),
            &[(&user).into()],
        )
        .await?;
    if host.as_i64().unwrap_or_default() == 0 {
        return Ok(HostPasswordOutcome::NotAHost);
    }
    let returned = conn
        .scalar(&set_password_sql(&q), &[(&application).into(), (&user).into(), (&hash).into(), (&salt).into()])
        .await?;
    Ok(if returned.as_i64() == Some(0) { HostPasswordOutcome::Changed } else { HostPasswordOutcome::NoMembershipUser })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_batch_names_dnns_tables_with_their_qualifier() {
        let sql = set_password_sql("dnn_");
        assert!(
            sql.contains("UPDATE dbo.[dnn_Users] SET UpdatePassword = 0 WHERE Username = @P2 AND IsSuperUser = 1;")
        );
        assert!(sql.contains("@PasswordFormat = 1") && sql.ends_with("COMMIT TRANSACTION; SELECT 0;"));
    }

    #[test]
    fn hashes_as_dnns_membership_provider() {
        let hasher = MembershipHasher;
        assert!(hasher.is_supported("SHA256") && !hasher.is_supported("Whirlpool"));
        let salt = hasher.new_salt();
        assert_eq!(
            hasher.hash("Secret-123", &salt, "SHA256"),
            dnnm_sql::membership::hash("Secret-123", &salt, "SHA256").unwrap()
        );
    }

    #[test]
    fn a_password_dnn_refuses_is_refused_first() {
        use use_case::HostPasswordChanger;
        let dnn = DnnHostPassword { settings: Settings::default() };
        assert_eq!(dnn.password_problem("short").as_deref(), Some("The password needs at least 7 characters."));
        assert_eq!(dnn.password_problem("Secret-123"), None);
        assert_eq!(change_host_password_done("shop", "host"), "'host' signs in to shop with the new password.");
    }

    #[test]
    fn a_site_without_membership_settings_is_refused_before_sql() {
        use use_case::HostPasswordChanger;
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("web.config"), "<configuration><system.web /></configuration>").unwrap();
        let dnn = DnnHostPassword { settings: Settings::default() };
        let database = DatabaseConnection::new(
            DatabaseKind::SqlServer,
            "nowhere",
            "shop",
            dnnm_application::use_cases::database_connection::SqlAuthentication::Windows,
        );
        let error = dnn.change_host_password(dir.path(), &database, "host", "Secret-123", &CancelToken::new());
        assert_eq!(error.unwrap_err(), "The site's web.config has no membership provider to go by.");
    }
}
