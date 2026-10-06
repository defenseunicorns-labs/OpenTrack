//! Mutual authentication of an external plugin: a shared secret, and a
//! challenge each way, before any other message.
//!
//! OpenTrack opens every connection with
//!
//! ```text
//! -> {"id":1,"method":"hello","params":{"protocol":"opentrack-plugin-v1","challenge":<Nc>}}
//! <- {"id":1,"result":{"proof":<HMAC(secret, "opentrack-plugin-v1 plugin\0" Nc Np)>,"challenge":<Np>}}
//! -> {"id":2,"method":"verify","params":{"proof":<HMAC(secret, "opentrack-plugin-v1 opentrack\0" Nc Np)>}}
//! <- {"id":2,"result":null}
//! ```
//!
//! `Nc` and `Np` are 32 random bytes each (standard base64 on the wire);
//! the key is the secret's UTF-8 bytes; the MAC is HMAC-SHA256. The plugin
//! proves itself first, over both challenges, and OpenTrack answers only
//! once that proof checks, so neither side can be made to sign something
//! for the other. The role label keeps a plugin's proof from ever passing
//! as OpenTrack's (and so a reflected challenge from working), and the
//! fixed-length nonces keep the input unambiguous.
//!
//! This authenticates both ends when the connection opens. It does not
//! encrypt what follows or protect it from change on the way: run external
//! plugins over a unix socket or a network you trust.

use aws_lc_rs::hmac;
use aws_lc_rs::rand::{SecureRandom, SystemRandom};

/// The protocol name, and the start of every MAC input.
pub const PROTOCOL: &str = "opentrack-plugin-v1";
/// Length of each challenge (bytes).
pub const NONCE_LEN: usize = 32;
/// Shortest secret accepted (characters), as for `OT_SESSION_SECRET`.
pub const MIN_SECRET_LEN: usize = 32;

/// Who a proof comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The plugin, answering OpenTrack's challenge.
    Plugin,
    /// OpenTrack, answering the plugin's.
    OpenTrack,
}

impl Role {
    fn label(self) -> &'static str {
        match self {
            Role::Plugin => "plugin",
            Role::OpenTrack => "opentrack",
        }
    }
}

/// What is MACed: `"opentrack-plugin-v1 <role>\0" || Nc || Np`.
pub fn mac_input(role: Role, opentrack_nonce: &[u8], plugin_nonce: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(PROTOCOL.len() + 12 + 2 * NONCE_LEN);
    m.extend_from_slice(PROTOCOL.as_bytes());
    m.push(b' ');
    m.extend_from_slice(role.label().as_bytes());
    m.push(0);
    m.extend_from_slice(opentrack_nonce);
    m.extend_from_slice(plugin_nonce);
    m
}

/// The proof `role` sends: HMAC-SHA256 over [`mac_input`].
pub fn proof(secret: &str, role: Role, opentrack_nonce: &[u8], plugin_nonce: &[u8]) -> Vec<u8> {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    hmac::sign(&key, &mac_input(role, opentrack_nonce, plugin_nonce))
        .as_ref()
        .to_vec()
}

/// Whether `tag` is `role`'s proof (compared in constant time).
pub fn verify(
    secret: &str,
    role: Role,
    opentrack_nonce: &[u8],
    plugin_nonce: &[u8],
    tag: &[u8],
) -> bool {
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    hmac::verify(&key, &mac_input(role, opentrack_nonce, plugin_nonce), tag).is_ok()
}

/// A fresh challenge from the FIPS module's DRBG.
pub fn nonce() -> [u8; NONCE_LEN] {
    let mut b = [0u8; NONCE_LEN];
    // The DRBG only fails if the module has failed its self-tests, after
    // which nothing it produces may be used.
    SystemRandom::new()
        .fill(&mut b)
        .expect("the FIPS module's random generator failed");
    b
}

/// Whether a secret (after `${env:NAME}` resolution) is long enough.
pub fn check_secret(secret: &str) -> Result<(), String> {
    if secret.chars().count() < MIN_SECRET_LEN {
        return Err(format!(
            "a plugin secret must be at least {MIN_SECRET_LEN} characters"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn the_mac_input_is_labelled_and_carries_both_challenges() {
        let m = mac_input(Role::Plugin, &[1; NONCE_LEN], &[2; NONCE_LEN]);
        let head = b"opentrack-plugin-v1 plugin\0";
        assert_eq!(&m[..head.len()], head);
        assert_eq!(&m[head.len()..head.len() + NONCE_LEN], &[1; NONCE_LEN]);
        assert_eq!(&m[head.len() + NONCE_LEN..], &[2; NONCE_LEN]);
        let o = mac_input(Role::OpenTrack, &[1; NONCE_LEN], &[2; NONCE_LEN]);
        assert!(o.starts_with(b"opentrack-plugin-v1 opentrack\0"));
    }

    #[test]
    fn a_proof_is_hmac_sha256_over_the_input() {
        // Known answer, as the Python SDK computes it:
        // hmac.new(S, b"opentrack-plugin-v1 plugin\0" + bytes(32) + bytes([255])*32, sha256)
        let p = proof(S, Role::Plugin, &[0; NONCE_LEN], &[255; NONCE_LEN]);
        assert_eq!(p.len(), 32);
        let hex: String = p.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, KNOWN);
    }

    const KNOWN: &str = "050e2b2ffe12cd43b2dc9fb61bd66add8e0b98e9b76c7c78756de49b0aad1daa";

    #[test]
    fn proofs_verify_only_with_the_same_secret_role_and_challenges() {
        let (c, p) = (nonce(), nonce());
        let tag = proof(S, Role::Plugin, &c, &p);
        assert!(verify(S, Role::Plugin, &c, &p, &tag));
        // Another secret, the other role (a reflection), or other challenges.
        assert!(!verify(&S.replace('0', "1"), Role::Plugin, &c, &p, &tag));
        assert!(!verify(S, Role::OpenTrack, &c, &p, &tag));
        assert!(!verify(S, Role::Plugin, &p, &c, &tag));
        assert!(!verify(S, Role::Plugin, &c, &p, &tag[..31]));
    }

    #[test]
    fn challenges_differ_and_short_secrets_are_refused() {
        assert_ne!(nonce(), nonce());
        assert!(check_secret("short").is_err());
        assert!(check_secret(S).is_ok());
    }
}
