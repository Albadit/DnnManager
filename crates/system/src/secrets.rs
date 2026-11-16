//! DNN Manager's secrets - `Settings/WindowsCredentialStore.cs`, `Settings/SecretProtector.cs` and the part of
//! `Settings/SettingsStore.cs` that keeps the sa password encrypted.
//!
//! - The default host password and the database server login's password are generic credentials of the signed-in user
//!   in the Windows Credential Manager, named `DnnManager/<name>` ([`CredentialStore`], [`names`]).
//! - The SQL Server sa password is a settings row (`sqlServer.saPassword`), encrypted with DPAPI for the current user as
//!   `dpapi:<base64>` with the WPF app's entropy, so a value either app wrote decrypts in the other ([`protect`]).
//!
//! No function here prints or logs a secret; errors say what failed, never the value.

use std::collections::BTreeMap;
use std::ffi::c_void;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use windows::Win32::Foundation::{ERROR_NOT_FOUND, HLOCAL, LocalFree};
use windows::Win32::Security::Credentials::{
    CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
    CredWriteW,
};
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::core::{PCWSTR, PWSTR};

use crate::win::wide;

/// The names secrets are kept under (`SecretNames`).
pub mod names {
    /// The host password new projects start with.
    pub const DEFAULT_HOST_PASSWORD: &str = "dnn-defaults/host-password";
    /// The password of the SQL Server login new projects use (Settings → Database server, SQL Server authentication).
    pub const DATABASE_SERVER_PASSWORD: &str = "database-server/password";
}

const TARGET_PREFIX: &str = "DnnManager/";
// CRED_MAX_CREDENTIAL_BLOB_SIZE: 5 * 512 bytes.
const MAX_BLOB_BYTES: usize = 2560;

/// The Credential Manager's name of the secret `name`: `DnnManager/<name>`.
pub fn target_name(name: &str) -> String {
    format!("{TARGET_PREFIX}{name}")
}

/// Why a secret can't be kept: a message for the user, never the secret.
fn too_long(secret: &str) -> Option<String> {
    (secret.encode_utf16().count() * 2 > MAX_BLOB_BYTES)
        .then(|| "The secret is too long for the Windows Credential Manager.".to_string())
}

/// The Windows Credential Manager of the signed-in user (`WindowsCredentialStore`): generic credentials named
/// `DnnManager/<name>`, which Control Panel → Credential Manager → Windows Credentials shows and removes. Windows
/// encrypts them for this user; nothing is written to the settings.
#[derive(Debug, Default, Clone, Copy)]
pub struct CredentialStore;

impl CredentialStore {
    /// The secret stored under `name`, or None when there is none.
    pub fn read(&self, name: &str) -> Option<String> {
        let target = wide(&target_name(name));
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: CredReadW hands back memory Windows allocated; it is read, then freed with CredFree.
        unsafe {
            CredReadW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None, &mut credential).ok()?;
            let c = &*credential;
            let value = if c.CredentialBlobSize == 0 || c.CredentialBlob.is_null() {
                String::new()
            } else {
                let units =
                    std::slice::from_raw_parts(c.CredentialBlob as *const u16, c.CredentialBlobSize as usize / 2);
                String::from_utf16_lossy(units)
            };
            CredFree(credential as *const c_void);
            Some(value)
        }
    }

    pub fn write(&self, name: &str, secret: &str) -> Result<(), String> {
        if let Some(problem) = too_long(secret) {
            return Err(problem);
        }
        let mut blob: Vec<u16> = secret.encode_utf16().collect();
        let mut target = wide(&target_name(name));
        let mut comment = wide("DNN Manager");
        let mut user = wide(&std::env::var("USERNAME").unwrap_or_default());
        let credential = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target.as_mut_ptr()),
            Comment: PWSTR(comment.as_mut_ptr()),
            CredentialBlobSize: (blob.len() * 2) as u32,
            CredentialBlob: blob.as_mut_ptr() as *mut u8,
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: PWSTR(user.as_mut_ptr()),
            ..Default::default()
        };
        // SAFETY: every pointer in the credential points into a buffer that lives until after the call.
        let result = unsafe { CredWriteW(&credential, 0) };
        // Don't leave the secret behind in memory.
        blob.iter_mut().for_each(|u| *u = 0);
        std::hint::black_box(&blob);
        result.map_err(|e| format!("Could not save it in the Windows Credential Manager: {}", e.message().trim()))
    }

    /// Removes it; Ok when there was nothing to remove.
    pub fn delete(&self, name: &str) -> Result<(), String> {
        let target = wide(&target_name(name));
        // SAFETY: a NUL-terminated name.
        match unsafe { CredDeleteW(PCWSTR(target.as_ptr()), CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ERROR_NOT_FOUND.to_hresult() => Ok(()),
            Err(e) => Err(format!("Could not remove it from the Windows Credential Manager: {}", e.message().trim())),
        }
    }
}

// ─── DPAPI (`SecretProtector`) ─────────────────────────────────────────────

const DPAPI_PREFIX: &str = "dpapi:";

// Ties the encrypted value to DNN Manager's settings, so another program's DPAPI data can't be passed off as it. The
// WPF app's own: a value it encrypted decrypts here, and the other way round.
const ENTROPY: &[u8] = b"DnnManager.settings.v1";

/// Whether `value` is an encrypted one (`dpapi:…`).
pub fn is_protected(value: &str) -> bool {
    value.starts_with(DPAPI_PREFIX)
}

/// `plain` encrypted with DPAPI for this Windows user on this PC: `dpapi:<base64>`.
pub fn protect(plain: &str) -> Result<String, String> {
    let bytes = dpapi(plain.as_bytes(), true).map_err(|e| format!("The secret can't be encrypted: {e}"))?;
    Ok(format!("{DPAPI_PREFIX}{}", BASE64.encode(bytes)))
}

/// The plain value of `value` - as it is when it isn't encrypted. An error when it was encrypted by another Windows
/// user or on another PC, or is damaged.
pub fn unprotect(value: &str) -> Result<String, String> {
    let Some(encoded) = value.strip_prefix(DPAPI_PREFIX) else { return Ok(value.to_string()) };
    let bytes = BASE64.decode(encoded).map_err(|_| "The encrypted value isn't valid base64.".to_string())?;
    let plain = dpapi(&bytes, false)?;
    String::from_utf8(plain).map_err(|_| "The decrypted value isn't text.".to_string())
}

fn dpapi(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let entropy = CRYPT_INTEGER_BLOB { cbData: ENTROPY.len() as u32, pbData: ENTROPY.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: the input blobs point to live slices Windows only reads; the output is copied, then freed with LocalFree.
    unsafe {
        let result = if encrypt {
            CryptProtectData(&input, PCWSTR::null(), Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptUnprotectData(&input, None, Some(&entropy), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        };
        result.map_err(|e| e.message().trim().to_string())?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        std::ptr::write_bytes(output.pbData, 0, output.cbData as usize);
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut c_void)));
        Ok(bytes)
    }
}

// ─── The settings' secret (`SettingsStore`) ──────────────────────────────────

/// The settings row that is kept encrypted: the container's sa password.
pub const SA_PASSWORD_KEY: &str = "sqlServer.saPassword";

/// The settings' sa password couldn't be decrypted (`SettingsException`): the message, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsSecretError {
    pub message: String,
    pub hints: Vec<String>,
}

/// `rows` as they are saved: the sa password encrypted (`SettingsStore.Rows`).
pub fn protect_rows(rows: &mut BTreeMap<String, String>) -> Result<(), String> {
    if let Some(password) = rows.get_mut(SA_PASSWORD_KEY)
        && !is_protected(password)
    {
        *password = protect(password)?;
    }
    Ok(())
}

/// `rows` as they were read, with the sa password decrypted (`SettingsStore.Parse`); `location` is the settings'
/// file, for the message.
pub fn unprotect_rows(rows: &mut BTreeMap<String, String>, location: &str) -> Result<(), SettingsSecretError> {
    if let Some(password) = rows.get_mut(SA_PASSWORD_KEY)
        && is_protected(password)
    {
        *password = unprotect(password).map_err(|_| SettingsSecretError {
            message: format!("The settings in {location} have an SA password that can't be decrypted."),
            hints: vec![
                "sqlServer.saPassword was encrypted by another Windows user or on another PC. Reset the settings, then enter the password again in Settings → Database server.".into(),
            ],
        })?;
    }
    Ok(())
}

/// Whether a saved value decrypts here - a reset keeps the sa password only then (`SettingsStore.Decrypts`).
pub fn decrypts(value: &str) -> bool {
    unprotect(value).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_named_under_dnn_manager() {
        assert_eq!(target_name(names::DEFAULT_HOST_PASSWORD), "DnnManager/dnn-defaults/host-password");
        assert_eq!(target_name(names::DATABASE_SERVER_PASSWORD), "DnnManager/database-server/password");
    }

    #[test]
    fn a_secret_longer_than_the_credential_manager_takes_is_refused_without_being_written() {
        assert!(too_long(&"x".repeat(1280)).is_none());
        let long = "x".repeat(1281);
        assert_eq!(
            CredentialStore.write("test/too-long", &long).unwrap_err(),
            "The secret is too long for the Windows Credential Manager."
        );
    }

    #[test]
    fn dpapi_round_trips_in_memory_with_the_prefix() {
        let protected = protect("Pa$$w0rd-é").unwrap();
        assert!(protected.starts_with("dpapi:"));
        assert!(!protected.contains("Pa$$w0rd"));
        assert_eq!(unprotect(&protected).unwrap(), "Pa$$w0rd-é");
        assert_eq!(unprotect("plain").unwrap(), "plain", "a value that isn't encrypted is as it is");
    }

    #[test]
    fn a_damaged_value_is_an_error_not_a_panic() {
        assert!(unprotect("dpapi:not base64!").is_err());
        assert!(unprotect("dpapi:AAAA").is_err());
        assert!(!decrypts("dpapi:AAAA"));
    }

    #[test]
    fn only_the_sa_password_row_is_encrypted_and_decrypted() {
        let mut rows = BTreeMap::from([
            (SA_PASSWORD_KEY.to_string(), "secret".to_string()),
            ("appearance.theme".to_string(), "dark".to_string()),
        ]);
        protect_rows(&mut rows).unwrap();
        assert!(is_protected(&rows[SA_PASSWORD_KEY]));
        assert_eq!(rows["appearance.theme"], "dark");
        let saved = rows[SA_PASSWORD_KEY].clone();
        protect_rows(&mut rows).unwrap();
        assert_eq!(rows[SA_PASSWORD_KEY], saved, "not encrypted twice");
        unprotect_rows(&mut rows, "db").unwrap();
        assert_eq!(rows[SA_PASSWORD_KEY], "secret");
    }

    #[test]
    fn an_sa_password_from_another_user_is_the_settings_error() {
        let mut rows = BTreeMap::from([(SA_PASSWORD_KEY.to_string(), "dpapi:AAAA".to_string())]);
        let error = unprotect_rows(&mut rows, r"C:\x\dnnmanager.db").unwrap_err();
        assert_eq!(error.message, r"The settings in C:\x\dnnmanager.db have an SA password that can't be decrypted.");
        assert!(error.hints[0].starts_with("sqlServer.saPassword was encrypted by another Windows user"));
    }

    // Writes, reads and removes a real credential: needs the user's Credential Manager, so it doesn't run by default.
    #[test]
    #[ignore = "writes to the Windows Credential Manager"]
    fn credential_manager_round_trip() {
        let store = CredentialStore;
        let name = "tests/round-trip";
        store.write(name, "s3cret-é").unwrap();
        assert_eq!(store.read(name).as_deref(), Some("s3cret-é"));
        store.delete(name).unwrap();
        assert_eq!(store.read(name), None);
        store.delete(name).unwrap();
    }
}
