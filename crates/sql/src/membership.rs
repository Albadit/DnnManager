//! Passwords as ASP.NET's `SqlMembershipProvider` - DNN's membership provider - stores them with
//! `passwordFormat="Hashed"` (`Dnn/MembershipPasswords.cs`): the hash of the salt followed by the password's UTF-16
//! bytes, or for a keyed (HMAC) algorithm the hash of the password with the salt as the key, both Base64. DNN 9 and 10
//! ship with SHA256; older sites often still have SHA1.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Algorithm {
    Sha1,
    Sha256,
    Sha384,
    Sha512,
    Md5,
    HmacSha1,
    HmacSha256,
    HmacSha384,
    HmacSha512,
}

// The names .NET Framework's HashAlgorithm.Create knows for these (case doesn't matter there either).
fn algorithm(name: &str) -> Option<Algorithm> {
    Some(match name.trim().to_uppercase().as_str() {
        "SHA1" | "SHA" | "SYSTEM.SECURITY.CRYPTOGRAPHY.SHA1" => Algorithm::Sha1,
        "SHA256" | "SHA-256" | "SYSTEM.SECURITY.CRYPTOGRAPHY.SHA256" => Algorithm::Sha256,
        "SHA384" | "SHA-384" | "SYSTEM.SECURITY.CRYPTOGRAPHY.SHA384" => Algorithm::Sha384,
        "SHA512" | "SHA-512" | "SYSTEM.SECURITY.CRYPTOGRAPHY.SHA512" => Algorithm::Sha512,
        "MD5" | "SYSTEM.SECURITY.CRYPTOGRAPHY.MD5" => Algorithm::Md5,
        "HMACSHA1" => Algorithm::HmacSha1,
        "HMACSHA256" => Algorithm::HmacSha256,
        "HMACSHA384" => Algorithm::HmacSha384,
        "HMACSHA512" => Algorithm::HmacSha512,
        _ => return None,
    })
}

/// The hash algorithms `membership/@hashAlgorithmType` can name that this knows.
pub fn is_supported(algorithm_name: &str) -> bool {
    algorithm(algorithm_name).is_some()
}

/// A new random salt, as SqlMembershipProvider makes them: 16 bytes, Base64.
pub fn new_salt() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the system's random numbers");
    STANDARD.encode(bytes)
}

/// The stored form of `password` with `salt` (Base64), as SqlMembershipProvider.EncodePassword makes it.
pub fn hash(password: &str, salt: &str, algorithm_name: &str) -> Result<String, String> {
    let password: Vec<u8> = password.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let salt = STANDARD.decode(salt).map_err(|_| "The salt isn't Base64.".to_string())?;
    let algorithm = algorithm(algorithm_name).ok_or_else(|| format!("Unknown hash algorithm '{algorithm_name}'."))?;
    let digest = match algorithm {
        // The keyed ones get the same default key length as on .NET Framework: 64 bytes, 128 for SHA384 and SHA512.
        Algorithm::HmacSha1 => keyed::<Hmac<Sha1>>(&key(&salt, 64), &password),
        Algorithm::HmacSha256 => keyed::<Hmac<Sha256>>(&key(&salt, 64), &password),
        Algorithm::HmacSha384 => keyed::<Hmac<Sha384>>(&key(&salt, 128), &password),
        Algorithm::HmacSha512 => keyed::<Hmac<Sha512>>(&key(&salt, 128), &password),
        plain => {
            let mut all = salt;
            all.extend_from_slice(&password);
            match plain {
                Algorithm::Sha1 => Sha1::digest(&all).to_vec(),
                Algorithm::Sha256 => Sha256::digest(&all).to_vec(),
                Algorithm::Sha384 => Sha384::digest(&all).to_vec(),
                Algorithm::Sha512 => Sha512::digest(&all).to_vec(),
                _ => md5::Md5::digest(&all).to_vec(),
            }
        }
    };
    Ok(STANDARD.encode(digest))
}

/// The key is the salt - cut to the key's length, or repeated to fill it (an empty salt leaves it zeros, as .NET's
/// default key would be).
fn key(salt: &[u8], length: usize) -> Vec<u8> {
    if salt.is_empty() {
        return vec![0; length];
    }
    salt.iter().copied().cycle().take(length).collect()
}

fn keyed<M: Mac + KeyInit>(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = <M as KeyInit>::new_from_slice(key).expect("HMAC takes a key of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn salt() -> String {
        STANDARD.encode((1..=16u8).collect::<Vec<_>>())
    }

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    #[test]
    fn keyed_algorithm_uses_the_salt_as_the_key() {
        // SqlMembershipProvider repeats the 16-byte salt to fill HMACSHA256's 64-byte key.
        let key: Vec<u8> = (0..4).flat_map(|_| 1..=16u8).collect();
        let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(&key).unwrap();
        mac.update(&utf16("pässword"));
        let expected = STANDARD.encode(mac.finalize().into_bytes());
        assert_eq!(hash("pässword", &salt(), "HMACSHA256").unwrap(), expected);

        let mut all: Vec<u8> = (1..=16u8).collect();
        all.extend_from_slice(&utf16("pässword"));
        assert_eq!(hash("pässword", &salt(), "SHA1").unwrap(), STANDARD.encode(Sha1::digest(&all)));
        assert!(!is_supported("Whirlpool"));
    }

    /// The answers .NET gives for the same input (computed with System.Security.Cryptography and Encoding.Unicode) -
    /// the bytes a DNN site compares a sign-in with.
    #[test]
    fn known_answers_match_dotnet() {
        assert_eq!(hash("pässword", &salt(), "SHA256").unwrap(), "G+KYvm+8w6F1jT7nSioT9qWZDJPjaom8hB2zA3GBvSw=");
        assert_eq!(hash("pässword", &salt(), "md5").unwrap(), "v3o/ej6Sd/etX0nYXVPwjA==");
        assert_eq!(hash("pässword", &salt(), "HMACSHA1").unwrap(), "3o9qRjW+7pKxRa4FtyMEMIvgKk0=");
        assert_eq!(
            hash("pässword", &salt(), "HMACSHA512").unwrap(),
            "O1+/wQK50dWXebhAH+33ioGxiiNR2Ej/kUNIyWn0fayiERXf2EZFSDxelthnMNuO0MqxbNpZV3v8kxkb21Zrow=="
        );
        assert_eq!(
            hash("x", &salt(), " sha-256 ").unwrap(),
            hash("x", &salt(), "System.Security.Cryptography.SHA256").unwrap()
        );
    }

    #[test]
    fn a_new_salt_is_16_random_bytes() {
        let a = new_salt();
        assert_eq!(STANDARD.decode(&a).unwrap().len(), 16);
        assert_ne!(a, new_salt());
    }

    #[test]
    fn every_named_algorithm_hashes_and_an_unknown_one_is_refused() {
        for name in ["SHA1", "SHA256", "SHA384", "SHA512", "MD5", "HMACSHA1", "HMACSHA256", "HMACSHA384", "HMACSHA512"]
        {
            assert!(is_supported(name));
            assert!(hash("x", &salt(), name).is_ok(), "{name}");
        }
        assert_eq!(hash("x", &salt(), "Whirlpool").unwrap_err(), "Unknown hash algorithm 'Whirlpool'.");
    }
}
