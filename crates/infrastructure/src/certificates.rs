//! The certificate of an https binding (`IisManager.Certificate`): from the machine's certificate store, by its
//! thumbprint - and, for a binding whose configuration doesn't note its certificate (IIS before 8.5, or one bound
//! with netsh), the thumbprint http.sys has for it: what `netsh http show sslcert` shows, asked of the HTTP Server
//! API instead, whose answer doesn't depend on Windows' language. Read-only.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dnnm_application::iis::{IisBindingDetails, IisCertificate};

/// FILETIME's 100 ns ticks since 1601 → a time.
pub fn from_filetime(low: u32, high: u32) -> SystemTime {
    let ticks = (u64::from(high) << 32) | u64::from(low);
    const UNIX_START: u64 = 116_444_736_000_000_000;
    match ticks.checked_sub(UNIX_START) {
        Some(since) => UNIX_EPOCH + Duration::from_micros(since / 10),
        None => UNIX_EPOCH,
    }
}

/// "AB12…" → bytes; None for text that isn't hex.
pub fn hex_bytes(hex: &str) -> Option<Vec<u8>> {
    let hex: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    if hex.is_empty() || !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect()
}

/// Fills in the certificate of each https binding: the thumbprint the configuration notes, else http.sys's.
pub fn complete(bindings: &mut [IisBindingDetails]) {
    for binding in bindings.iter_mut().filter(|b| b.protocol.eq_ignore_ascii_case("https")) {
        if binding.certificate_hash.is_none()
            && let Some((hash, store)) = http_sys_binding(binding)
        {
            binding.certificate_hash = Some(hash);
            binding.certificate_store = Some(store);
        }
        if let (Some(hash), Some(store)) = (&binding.certificate_hash, &binding.certificate_store) {
            binding.certificate = find(hash, store);
        }
    }
}

/// The certificate with `thumbprint` in the machine's `store` (My, WebHosting); None when it isn't there.
#[cfg(windows)]
pub fn find(thumbprint: &str, store: &str) -> Option<IisCertificate> {
    use windows::Win32::Security::Cryptography::*;
    use windows::core::HSTRING;

    let mut hash = hex_bytes(thumbprint)?;
    let name = HSTRING::from(store);
    // SAFETY: the store is opened read-only and closed again; the context found is freed; the buffers' lengths are
    // passed with them.
    unsafe {
        let handle = CertOpenStore(
            CERT_STORE_PROV_SYSTEM_W,
            CERT_QUERY_ENCODING_TYPE(0),
            None,
            CERT_OPEN_STORE_FLAGS(
                CERT_SYSTEM_STORE_LOCAL_MACHINE | CERT_STORE_READONLY_FLAG.0 | CERT_STORE_OPEN_EXISTING_FLAG.0,
            ),
            Some(name.as_ptr() as *const _),
        )
        .ok()?;
        let blob = CRYPT_INTEGER_BLOB { cbData: hash.len() as u32, pbData: hash.as_mut_ptr() };
        let context = CertFindCertificateInStore(
            handle,
            X509_ASN_ENCODING | PKCS_7_ASN_ENCODING,
            0,
            CERT_FIND_HASH,
            Some(&blob as *const _ as *const _),
            None,
        );
        let found = if context.is_null() {
            None
        } else {
            let text = |flags: u32| {
                let mut buffer = [0u16; 512];
                let len = CertGetNameStringW(context, CERT_NAME_SIMPLE_DISPLAY_TYPE, flags, None, Some(&mut buffer));
                String::from_utf16_lossy(&buffer[..(len as usize).saturating_sub(1).min(buffer.len())])
            };
            let mut size = 0u32;
            let friendly = if CertGetCertificateContextProperty(context, CERT_FRIENDLY_NAME_PROP_ID, None, &mut size)
                .is_ok()
                && size > 2
            {
                let mut buffer = vec![0u16; size as usize / 2];
                CertGetCertificateContextProperty(
                    context,
                    CERT_FRIENDLY_NAME_PROP_ID,
                    Some(buffer.as_mut_ptr() as *mut _),
                    &mut size,
                )
                .ok()
                .map(|_| String::from_utf16_lossy(&buffer).trim_end_matches('\0').to_string())
                .filter(|f| !f.is_empty())
            } else {
                None
            };
            let info = &*(*context).pCertInfo;
            let certificate = IisCertificate {
                subject: text(0),
                friendly_name: friendly,
                issuer: text(CERT_NAME_ISSUER_FLAG),
                not_before: from_filetime(info.NotBefore.dwLowDateTime, info.NotBefore.dwHighDateTime),
                not_after: from_filetime(info.NotAfter.dwLowDateTime, info.NotAfter.dwHighDateTime),
            };
            let _ = CertFreeCertificateContext(Some(context));
            Some(certificate)
        };
        let _ = CertCloseStore(Some(handle), 0);
        found
    }
}

#[cfg(not(windows))]
pub fn find(_thumbprint: &str, _store: &str) -> Option<IisCertificate> {
    None
}

/// The IPv4 address http.sys keys a binding's certificate by: "*" (and an IPv6 address, which DNN sites don't use
/// for https) is 0.0.0.0, every address.
fn ipv4(address: &str) -> [u8; 4] {
    address.parse::<std::net::Ipv4Addr>().map(|a| a.octets()).unwrap_or([0, 0, 0, 0])
}

/// The thumbprint (upper-case hex) and store http.sys has for the binding - by host name and port for an SNI
/// binding, by address and port otherwise (then by every address). None when it has none, or can't be asked.
#[cfg(windows)]
pub fn http_sys_binding(binding: &IisBindingDetails) -> Option<(String, String)> {
    use windows::Win32::Networking::HttpServer::*;
    use windows::Win32::Networking::WinSock::{AF_INET, SOCKADDR_IN, SOCKADDR_STORAGE};
    use windows::core::{HSTRING, PWSTR};

    let port = binding.port?;
    let version = HTTPAPI_VERSION { HttpApiMajorVersion: 1, HttpApiMinorVersion: 0 };
    // SAFETY: HttpInitialize/HttpTerminate pair; every query gets its input struct and an output buffer of the length
    // passed, aligned for the struct it is read as.
    unsafe {
        if HttpInitialize(version, HTTP_INITIALIZE_CONFIG, None) != 0 {
            return None;
        }
        let sockaddr = |octets: [u8; 4]| {
            let mut address = SOCKADDR_IN { sin_family: AF_INET, sin_port: port.to_be(), ..Default::default() };
            address.sin_addr.S_un.S_addr = u32::from_ne_bytes(octets);
            address
        };
        let query = |id: HTTP_SERVICE_CONFIG_ID, input: *const core::ffi::c_void, length: u32| -> Option<Vec<u64>> {
            let mut needed = 0u32;
            let first = HttpQueryServiceConfiguration(None, id, Some(input), length, None, 0, Some(&mut needed), None);
            // ERROR_INSUFFICIENT_BUFFER: there is an entry, this long.
            if first != 122 || needed == 0 {
                return None;
            }
            let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
            let result = HttpQueryServiceConfiguration(
                None,
                id,
                Some(input),
                length,
                Some(buffer.as_mut_ptr() as *mut _),
                needed,
                Some(&mut needed),
                None,
            );
            (result == 0).then_some(buffer)
        };
        let read = |param: &HTTP_SERVICE_CONFIG_SSL_PARAM| {
            let hash = std::slice::from_raw_parts(param.pSslHash as *const u8, param.SslHashLength as usize);
            let hex: String = hash.iter().map(|b| format!("{b:02X}")).collect();
            let store = if param.pSslCertStoreName.is_null() {
                String::new()
            } else {
                param.pSslCertStoreName.to_string().unwrap_or_default()
            };
            (hex, if store.is_empty() { "My".to_string() } else { store })
        };

        let found = if binding.sni && !binding.host.is_empty() {
            let host = HSTRING::from(binding.host.as_str());
            let mut key = HTTP_SERVICE_CONFIG_SSL_SNI_QUERY {
                QueryDesc: HttpServiceConfigQueryExact,
                KeyDesc: HTTP_SERVICE_CONFIG_SSL_SNI_KEY {
                    IpPort: SOCKADDR_STORAGE::default(),
                    Host: PWSTR(host.as_ptr() as *mut u16),
                },
                dwToken: 0,
            };
            let address = sockaddr([0, 0, 0, 0]);
            std::ptr::copy_nonoverlapping(
                &address as *const SOCKADDR_IN as *const u8,
                &mut key.KeyDesc.IpPort as *mut SOCKADDR_STORAGE as *mut u8,
                size_of::<SOCKADDR_IN>(),
            );
            query(HttpServiceConfigSslSniCertInfo, &key as *const _ as *const _, size_of_val(&key) as u32)
                .map(|buffer| read(&(*(buffer.as_ptr() as *const HTTP_SERVICE_CONFIG_SSL_SNI_SET)).ParamDesc))
        } else {
            let mut found = None;
            for octets in [ipv4(&binding.address), [0, 0, 0, 0]] {
                let mut address = sockaddr(octets);
                let key = HTTP_SERVICE_CONFIG_SSL_QUERY {
                    QueryDesc: HttpServiceConfigQueryExact,
                    KeyDesc: HTTP_SERVICE_CONFIG_SSL_KEY { pIpPort: &mut address as *mut SOCKADDR_IN as *mut _ },
                    dwToken: 0,
                };
                found = query(HttpServiceConfigSSLCertInfo, &key as *const _ as *const _, size_of_val(&key) as u32)
                    .map(|buffer| read(&(*(buffer.as_ptr() as *const HTTP_SERVICE_CONFIG_SSL_SET)).ParamDesc));
                if found.is_some() {
                    break;
                }
            }
            found
        };
        HttpTerminate(HTTP_INITIALIZE_CONFIG, None);
        found.filter(|(hash, _)| !hash.is_empty())
    }
}

#[cfg(not(windows))]
pub fn http_sys_binding(_binding: &IisBindingDetails) -> Option<(String, String)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_hex_and_filetimes() {
        assert_eq!(hex_bytes("ab 12"), Some(vec![0xab, 0x12]));
        assert_eq!(hex_bytes("abc"), None);
        assert_eq!(hex_bytes("zz"), None);
        // 2001-01-01T00:00:00Z.
        let at = from_filetime(0xC89D_C000, 0x01C0_7385);
        assert_eq!(at.duration_since(UNIX_EPOCH).unwrap().as_secs() / 86_400, 11_323);
    }

    #[test]
    fn a_thumbprint_nowhere_is_none_not_an_error() {
        assert_eq!(find("00112233445566778899AABBCCDDEEFF00112233", "My"), None);
        assert_eq!(find("not hex", "My"), None);
        assert_eq!(find("0011", "NoSuchStoreDnnm"), None);
    }

    #[test]
    fn http_sys_answers_without_a_binding() {
        // Read-only: asks http.sys about a port nothing uses.
        let binding = IisBindingDetails::parse("https", "*:1:", false);
        assert_eq!(http_sys_binding(&binding), None);
        let sni = IisBindingDetails::parse("https", "*:1:nothing.invalid", true);
        assert_eq!(http_sys_binding(&sni), None);
    }
}
