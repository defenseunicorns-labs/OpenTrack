//! FIPS 140-3: all cryptography goes through the AWS-LC validated module.
//!
//! [`init`] turns the module on (its power-on self-tests run then) and makes
//! its FIPS configuration the process's only TLS provider; the process stops
//! if either fails. Passwords are hashed with PBKDF2-HMAC-SHA256 (SP 800-132)
//! and every secret comes from the module's DRBG.

use std::num::NonZeroU32;

use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD as B64;

/// Refuse to run unless the validated module is active and TLS uses it.
pub fn init() -> anyhow::Result<()> {
    aws_lc_rs::try_fips_mode()
        .map_err(|e| anyhow::anyhow!("the AWS-LC FIPS module is not in FIPS mode: {e}"))?;
    let provider = rustls::crypto::default_fips_provider();
    anyhow::ensure!(
        provider.fips(),
        "the TLS provider is not in its FIPS configuration"
    );
    // Another provider may only be installed if something ran before us;
    // then that one must be FIPS too.
    let _ = provider.install_default();
    let installed = rustls::crypto::CryptoProvider::get_default().is_some_and(|p| p.fips());
    anyhow::ensure!(installed, "a non-FIPS TLS provider was installed first");
    Ok(())
}

/// `n` random bytes from the module's DRBG.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // The DRBG only fails if the module has failed its self-tests, after
    // which nothing it produces may be used.
    SystemRandom::new()
        .fill(&mut b)
        .expect("the FIPS module's random generator failed");
    b
}

/// A random secret or identifier: `N` bytes as lowercase hex.
pub fn random_hex<const N: usize>() -> String {
    random_bytes::<N>()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A random version-4 UUID (for ids that other code expects as UUIDs).
pub fn uuid_v4() -> String {
    uuid::Builder::from_random_bytes(random_bytes::<16>())
        .into_uuid()
        .to_string()
}

const PBKDF2: aws_lc_rs::pbkdf2::Algorithm = aws_lc_rs::pbkdf2::PBKDF2_HMAC_SHA256;
/// OWASP's 2023 figure for PBKDF2-HMAC-SHA256.
pub const PBKDF2_ITERATIONS: u32 = 600_000;
const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;
const PREFIX: &str = "$pbkdf2-sha256$";

/// `$pbkdf2-sha256$i=<iterations>$<salt>$<hash>` (base64, no padding).
pub fn hash_password(password: &str) -> String {
    let salt = random_bytes::<SALT_LEN>();
    let mut out = [0u8; HASH_LEN];
    let iterations = NonZeroU32::new(PBKDF2_ITERATIONS).expect("non-zero");
    aws_lc_rs::pbkdf2::derive(PBKDF2, iterations, &salt, password.as_bytes(), &mut out);
    format!(
        "{PREFIX}i={PBKDF2_ITERATIONS}${}${}",
        B64.encode(salt),
        B64.encode(out)
    )
}

/// Whether `password` matches a PBKDF2 hash (in constant time). `None` when
/// `hash` is not a PBKDF2 hash at all.
pub fn verify_password(password: &str, hash: &str) -> Option<bool> {
    let rest = hash.strip_prefix(PREFIX)?;
    let mut parts = rest.split('$');
    let (Some(iter), Some(salt), Some(want), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Some(false);
    };
    let parsed = iter
        .strip_prefix("i=")
        .and_then(|i| i.parse::<u32>().ok())
        .and_then(NonZeroU32::new)
        .zip(B64.decode(salt).ok())
        .zip(B64.decode(want).ok());
    let Some(((iterations, salt), want)) = parsed else {
        return Some(false);
    };
    Some(aws_lc_rs::pbkdf2::verify(PBKDF2, iterations, &salt, password.as_bytes(), &want).is_ok())
}

/// Whether a stored hash should be replaced at the next successful sign-in:
/// anything but PBKDF2 at today's work factor.
pub fn needs_rehash(hash: &str) -> bool {
    !hash.starts_with(&format!("{PREFIX}i={PBKDF2_ITERATIONS}$"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_module_is_in_fips_mode() {
        init().unwrap();
        assert!(aws_lc_rs::try_fips_mode().is_ok());
    }

    #[test]
    fn pbkdf2_hashes_verify_and_are_salted() {
        let h = hash_password("correct horse battery");
        assert!(h.starts_with("$pbkdf2-sha256$i=600000$"));
        assert_eq!(verify_password("correct horse battery", &h), Some(true));
        assert_eq!(verify_password("wrong horse battery", &h), Some(false));
        assert_ne!(h, hash_password("correct horse battery"));
        assert!(!needs_rehash(&h));
        assert_eq!(verify_password("x", "$argon2id$v=19$..."), None);
        assert_eq!(
            verify_password("x", "$pbkdf2-sha256$i=0$AA$AA"),
            Some(false)
        );
        assert!(needs_rehash("$argon2id$v=19$m=19456,t=2,p=1$abc$def"));
        assert!(needs_rehash("$pbkdf2-sha256$i=1000$AA$AA"));
    }

    #[test]
    fn a_known_answer() {
        // RFC 7914 §11 PBKDF2-HMAC-SHA256 vector: P="passwd", S="salt", c=1.
        let mut out = [0u8; 64];
        aws_lc_rs::pbkdf2::derive(
            PBKDF2,
            NonZeroU32::new(1).unwrap(),
            b"salt",
            b"passwd",
            &mut out,
        );
        assert_eq!(&out[..8], &[0x55, 0xac, 0x04, 0x6e, 0x56, 0xe3, 0x08, 0x9f]);
    }

    #[test]
    fn random_values_differ() {
        assert_ne!(random_hex::<16>(), random_hex::<16>());
        assert_eq!(random_hex::<32>().len(), 64);
        assert_eq!(
            uuid::Uuid::parse_str(&uuid_v4()).unwrap().get_version_num(),
            4
        );
    }
}
