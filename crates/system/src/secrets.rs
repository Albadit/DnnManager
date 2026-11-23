//! DNN Manager's secrets, the same on Windows, macOS and Linux.
//!
//! - Passwords kept on their own - the default host password and the database server login's - are in the OS's
//!   secure store for the signed-in user, named `DnnManager/<name>` ([`CredentialStore`], [`names`]): the Windows
//!   Credential Manager, the macOS Keychain, or the Secret Service on Linux (GNOME Keyring, KWallet).
//! - The SQL Server sa password is a settings row (`sqlServer.saPassword`), encrypted with AES-256-GCM as
//!   `aesgcm:<base64 of nonce and ciphertext>` ([`protect`]). Its key - 32 random bytes made the first time one is
//!   needed - is kept in that same store ([`KEY_NAME`]), never in the settings.
//! - A value the Windows app encrypted before with DPAPI (`dpapi:<base64>`) still decrypts on Windows, and is saved
//!   with AES-256-GCM the next time the settings are saved.
//!
//! No function here prints or logs a secret; errors say what failed, never the value.

use std::collections::BTreeMap;
use std::sync::Mutex;

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

pub use store::CredentialStore;

/// The names secrets are kept under (`SecretNames`).
pub mod names {
    /// The host password new projects start with.
    pub const DEFAULT_HOST_PASSWORD: &str = "dnn-defaults/host-password";
    /// The password of the SQL Server login new projects use (Settings → Database server, SQL Server authentication).
    pub const DATABASE_SERVER_PASSWORD: &str = "database-server/password";
}

/// The name of the settings' encryption key in the OS's store.
pub const KEY_NAME: &str = "settings/encryption-key";

const TARGET_PREFIX: &str = "DnnManager/";

/// The store's name of the secret `name`: `DnnManager/<name>`.
pub fn target_name(name: &str) -> String {
    format!("{TARGET_PREFIX}{name}")
}

/// The OS's store, as the messages name it.
pub fn store_name() -> &'static str {
    if cfg!(windows) {
        "the Windows Credential Manager"
    } else if cfg!(target_os = "macos") {
        "the macOS Keychain"
    } else {
        "the keyring (Secret Service)"
    }
}

// ─── The store: Windows ───────────────────────────────────────────────────

#[cfg(windows)]
mod store {
    use std::ffi::c_void;

    use windows::Win32::Foundation::ERROR_NOT_FOUND;
    use windows::Win32::Security::Credentials::{
        CRED_FLAGS, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree, CredReadW,
        CredWriteW,
    };
    use windows::core::{PCWSTR, PWSTR};

    use super::target_name;
    use crate::win::wide;

    // CRED_MAX_CREDENTIAL_BLOB_SIZE: 5 * 512 bytes.
    const MAX_BLOB_BYTES: usize = 2560;

    /// Why a secret can't be kept: a message for the user, never the secret.
    pub(super) fn too_long(secret: &str) -> Option<String> {
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
}

// ─── The store: macOS and Linux ─────────────────────────────────────────────

#[cfg(not(windows))]
mod store {
    use super::{TARGET_PREFIX, store_name};

    /// The macOS Keychain or Linux's Secret Service of the signed-in user, through the `keyring` crate: generic
    /// passwords of the service `DnnManager`, each under its name (Keychain Access lists them as `DnnManager`).
    #[derive(Debug, Default, Clone, Copy)]
    pub struct CredentialStore;

    fn entry(name: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(TARGET_PREFIX.trim_end_matches('/'), name)
            .map_err(|e| format!("Could not open {}: {e}", store_name()))
    }

    impl CredentialStore {
        /// The secret stored under `name`, or None when there is none (or the store can't be reached).
        pub fn read(&self, name: &str) -> Option<String> {
            match entry(name).ok()?.get_password() {
                Ok(secret) => Some(secret),
                Err(keyring::Error::NoEntry) => None,
                Err(e) => {
                    log::warn!("{} couldn't be read from {}: {e}", name, store_name());
                    None
                }
            }
        }

        pub fn write(&self, name: &str, secret: &str) -> Result<(), String> {
            entry(name)?.set_password(secret).map_err(|e| {
                let hint = if cfg!(target_os = "macos") {
                    ""
                } else {
                    " - DNN Manager needs a keyring service running (GNOME Keyring or KWallet)"
                };
                format!("Could not save it in {}: {e}{hint}", store_name())
            })
        }

        /// Removes it; Ok when there was nothing to remove.
        pub fn delete(&self, name: &str) -> Result<(), String> {
            match entry(name)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(format!("Could not remove it from {}: {e}", store_name())),
            }
        }
    }
}

// ─── The cipher: AES-256-GCM ─────────────────────────────────────────────────

const AES_PREFIX: &str = "aesgcm:";
const DPAPI_PREFIX: &str = "dpapi:";
const NONCE_BYTES: usize = 12;

/// Whether `value` is an encrypted one (`aesgcm:…`, or `dpapi:…` from before).
pub fn is_protected(value: &str) -> bool {
    value.starts_with(AES_PREFIX) || value.starts_with(DPAPI_PREFIX)
}

/// `plain` encrypted with the settings' key: `aesgcm:<base64>`. The key is made the first time; an error when the
/// OS's store can't keep it.
pub fn protect(plain: &str) -> Result<String, String> {
    let key = settings_key(true)?;
    seal(&key, plain)
}

/// The plain value of `value` - as it is when it isn't encrypted. An error when the key isn't in this user's store
/// (another user, another computer), when it was encrypted with DPAPI off Windows, or when it is damaged.
pub fn unprotect(value: &str) -> Result<String, String> {
    if let Some(encoded) = value.strip_prefix(AES_PREFIX) {
        return open(&settings_key(false)?, encoded);
    }
    let Some(encoded) = value.strip_prefix(DPAPI_PREFIX) else { return Ok(value.to_string()) };
    unprotect_dpapi(encoded)
}

/// `plain` with a fresh random nonce: `aesgcm:` and the base64 of the nonce, then the ciphertext with its tag.
fn seal(key: &[u8; 32], plain: &str) -> Result<String, String> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let sealed = cipher.encrypt(&nonce, plain.as_bytes()).map_err(|_| "The secret can't be encrypted.".to_string())?;
    let mut bytes = nonce.to_vec();
    bytes.extend_from_slice(&sealed);
    Ok(format!("{AES_PREFIX}{}", BASE64.encode(bytes)))
}

/// What [`seal`] made, back - an error when it was changed, or sealed with another key.
fn open(key: &[u8; 32], encoded: &str) -> Result<String, String> {
    let bytes = BASE64.decode(encoded).map_err(|_| "The encrypted value isn't valid base64.".to_string())?;
    if bytes.len() < NONCE_BYTES + 16 {
        return Err("The encrypted value is too short.".into());
    }
    let (nonce, sealed) = bytes.split_at(NONCE_BYTES);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce), sealed)
        .map_err(|_| "The encrypted value doesn't decrypt with this user's key - or it was changed.".to_string())?;
    String::from_utf8(plain).map_err(|_| "The decrypted value isn't text.".to_string())
}

/// The key read once is kept for the session: the store (the Keychain above all) isn't asked at every save.
static KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);

/// The settings' key from the OS's store; `create`: made and stored when there is none yet.
fn settings_key(create: bool) -> Result<[u8; 32], String> {
    let mut cached = KEY.lock().map_err(|_| "The settings' key is unavailable.".to_string())?;
    if let Some(key) = *cached {
        return Ok(key);
    }
    let store = CredentialStore;
    let key = match store.read(KEY_NAME) {
        Some(stored) => decode_key(&stored)?,
        None if create => {
            let key: [u8; 32] = Aes256Gcm::generate_key(&mut OsRng).into();
            store.write(KEY_NAME, &BASE64.encode(key))?;
            // Read back: a store that took it without keeping it would leave values no one can decrypt.
            match store.read(KEY_NAME).map(|s| decode_key(&s)) {
                Some(Ok(kept)) if kept == key => key,
                _ => return Err(format!("The settings' key couldn't be kept in {}.", store_name())),
            }
        }
        None => return Err(format!("The settings' key isn't in {} for this user.", store_name())),
    };
    *cached = Some(key);
    Ok(key)
}

fn decode_key(stored: &str) -> Result<[u8; 32], String> {
    let bytes = BASE64.decode(stored.trim()).map_err(|_| "The settings' key in the store is damaged.".to_string())?;
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| "The settings' key in the store is damaged.".to_string())
}

// ─── DPAPI, from before (Windows) ────────────────────────────────────────────

// Ties a DPAPI value to DNN Manager's settings, as the WPF app and the earlier Rust builds encrypted it.
#[cfg(windows)]
const ENTROPY: &[u8] = b"DnnManager.settings.v1";

#[cfg(windows)]
fn unprotect_dpapi(encoded: &str) -> Result<String, String> {
    let bytes = BASE64.decode(encoded).map_err(|_| "The encrypted value isn't valid base64.".to_string())?;
    let plain = dpapi(&bytes, false)?;
    String::from_utf8(plain).map_err(|_| "The decrypted value isn't text.".to_string())
}

#[cfg(not(windows))]
fn unprotect_dpapi(_encoded: &str) -> Result<String, String> {
    Err("The value was encrypted by DNN Manager on Windows (DPAPI) and can't be decrypted on this system.".into())
}

#[cfg(windows)]
fn dpapi(data: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    };
    use windows::core::PCWSTR;
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
        let _ = LocalFree(Some(HLOCAL(output.pbData as *mut std::ffi::c_void)));
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
    protect_rows_with(rows, protect)
}

fn protect_rows_with(
    rows: &mut BTreeMap<String, String>,
    protect: impl Fn(&str) -> Result<String, String>,
) -> Result<(), String> {
    if let Some(password) = rows.get_mut(SA_PASSWORD_KEY)
        && !password.is_empty()
        && !is_protected(password)
    {
        *password = protect(password)?;
    }
    Ok(())
}

/// `rows` as they were read, with the sa password decrypted (`SettingsStore.Parse`); `location` is the settings'
/// file, for the message.
pub fn unprotect_rows(rows: &mut BTreeMap<String, String>, location: &str) -> Result<(), SettingsSecretError> {
    unprotect_rows_with(rows, location, unprotect)
}

fn unprotect_rows_with(
    rows: &mut BTreeMap<String, String>,
    location: &str,
    unprotect: impl Fn(&str) -> Result<String, String>,
) -> Result<(), SettingsSecretError> {
    if let Some(password) = rows.get_mut(SA_PASSWORD_KEY)
        && is_protected(password)
    {
        *password = unprotect(password).map_err(|_| SettingsSecretError {
            message: format!("The settings in {location} have an SA password that can't be decrypted."),
            hints: vec![format!(
                "sqlServer.saPassword was encrypted by another user or on another computer, or its key is no longer in {}. Reset the settings, then enter the password again in Settings → Database server.",
                store_name()
            )],
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

    const KEY_A: [u8; 32] = [7; 32];
    const KEY_B: [u8; 32] = [9; 32];

    #[test]
    fn credentials_are_named_under_dnn_manager() {
        assert_eq!(target_name(names::DEFAULT_HOST_PASSWORD), "DnnManager/dnn-defaults/host-password");
        assert_eq!(target_name(names::DATABASE_SERVER_PASSWORD), "DnnManager/database-server/password");
        assert_eq!(target_name(KEY_NAME), "DnnManager/settings/encryption-key");
    }

    #[test]
    fn aes_gcm_round_trips_with_a_fresh_nonce_each_time() {
        let one = seal(&KEY_A, "Pa$$w0rd-é").unwrap();
        let two = seal(&KEY_A, "Pa$$w0rd-é").unwrap();
        assert!(one.starts_with("aesgcm:") && is_protected(&one));
        assert!(!one.contains("Pa$$w0rd"));
        assert_ne!(one, two, "a nonce is never used twice");
        assert_eq!(open(&KEY_A, &one["aesgcm:".len()..]).unwrap(), "Pa$$w0rd-é");
        assert_eq!(open(&KEY_A, &seal(&KEY_A, "").unwrap()["aesgcm:".len()..]).unwrap(), "");
    }

    #[test]
    fn another_key_or_a_changed_value_doesnt_decrypt() {
        let sealed = seal(&KEY_A, "secret").unwrap();
        let encoded = &sealed["aesgcm:".len()..];
        assert!(open(&KEY_B, encoded).is_err(), "another user's key");
        let mut bytes = BASE64.decode(encoded).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(open(&KEY_A, &BASE64.encode(bytes)).is_err(), "the tag catches a change");
        assert!(open(&KEY_A, "not base64!").is_err());
        assert!(open(&KEY_A, "AAAA").is_err(), "too short");
    }

    #[test]
    fn a_plain_value_is_as_it_is_and_a_damaged_one_is_an_error() {
        assert_eq!(unprotect("plain").unwrap(), "plain");
        assert!(!is_protected("plain"));
        assert!(is_protected("dpapi:AAAA"));
        assert!(unprotect("dpapi:not base64!").is_err());
        assert!(unprotect("dpapi:AAAA").is_err());
        assert!(!decrypts("dpapi:AAAA"));
    }

    #[test]
    fn a_stored_key_must_be_32_bytes_of_base64() {
        assert_eq!(decode_key(&BASE64.encode(KEY_A)).unwrap(), KEY_A);
        assert!(decode_key("AAAA").is_err());
        assert!(decode_key("not base64!").is_err());
    }

    #[test]
    fn only_the_sa_password_row_is_encrypted_and_decrypted() {
        let seal_a = |p: &str| seal(&KEY_A, p);
        let open_a = |v: &str| open(&KEY_A, v.strip_prefix("aesgcm:").unwrap_or(v));
        let mut rows = BTreeMap::from([
            (SA_PASSWORD_KEY.to_string(), "secret".to_string()),
            ("appearance.theme".to_string(), "dark".to_string()),
        ]);
        protect_rows_with(&mut rows, seal_a).unwrap();
        assert!(rows[SA_PASSWORD_KEY].starts_with("aesgcm:"));
        assert_eq!(rows["appearance.theme"], "dark");
        let saved = rows[SA_PASSWORD_KEY].clone();
        protect_rows_with(&mut rows, seal_a).unwrap();
        assert_eq!(rows[SA_PASSWORD_KEY], saved, "not encrypted twice");
        unprotect_rows_with(&mut rows, "db", open_a).unwrap();
        assert_eq!(rows[SA_PASSWORD_KEY], "secret");
        // An empty password stays empty: nothing to keep.
        let mut empty = BTreeMap::from([(SA_PASSWORD_KEY.to_string(), String::new())]);
        protect_rows_with(&mut empty, |_| Err("not called".into())).unwrap();
        assert_eq!(empty[SA_PASSWORD_KEY], "");
    }

    #[test]
    fn an_sa_password_that_doesnt_decrypt_is_the_settings_error() {
        let mut rows = BTreeMap::from([(SA_PASSWORD_KEY.to_string(), "aesgcm:AAAA".to_string())]);
        let error = unprotect_rows_with(&mut rows, r"C:\x\dnnmanager.db", |_| Err("no".into())).unwrap_err();
        assert_eq!(error.message, r"The settings in C:\x\dnnmanager.db have an SA password that can't be decrypted.");
        assert!(error.hints[0].starts_with("sqlServer.saPassword was encrypted by another user or on another computer"));
    }

    /// A value the Windows app saved with DPAPI still decrypts.
    #[cfg(windows)]
    #[test]
    fn dpapi_values_from_before_still_decrypt_on_windows() {
        let old = format!("dpapi:{}", BASE64.encode(dpapi("Old-Sa-1".as_bytes(), true).unwrap()));
        assert!(is_protected(&old));
        assert_eq!(unprotect(&old).unwrap(), "Old-Sa-1");
    }

    #[cfg(not(windows))]
    #[test]
    fn dpapi_values_say_they_are_windows_only() {
        assert!(unprotect("dpapi:AAAA").unwrap_err().contains("on Windows (DPAPI)"));
    }

    #[cfg(windows)]
    #[test]
    fn a_secret_longer_than_the_credential_manager_takes_is_refused_without_being_written() {
        assert!(store::too_long(&"x".repeat(1280)).is_none());
        let long = "x".repeat(1281);
        assert_eq!(
            CredentialStore.write("test/too-long", &long).unwrap_err(),
            "The secret is too long for the Windows Credential Manager."
        );
    }

    // Writes, reads and removes a real secret in the OS's store, so it doesn't run by default.
    #[test]
    #[ignore = "writes to the OS's credential store"]
    fn credential_store_round_trip() {
        let store = CredentialStore;
        let name = "tests/round-trip";
        store.write(name, "s3cret-é").unwrap();
        assert_eq!(store.read(name).as_deref(), Some("s3cret-é"));
        store.delete(name).unwrap();
        assert_eq!(store.read(name), None);
        store.delete(name).unwrap();
    }
}
