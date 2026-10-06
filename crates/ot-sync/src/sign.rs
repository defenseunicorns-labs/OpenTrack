//! Signed sync messages (`docs/sync-icd.md`, *Signature*).
//!
//! Every node has one Ed25519 key pair, made in the FIPS module (AWS-LC's
//! Ed25519 is FIPS 186-5 EdDSA and approved in its FIPS mode). A node signs
//! each message once, whoever it is for, and appends a 4-byte key id and the
//! 64-byte signature. A receiver checks the signature against the public key
//! an admin pinned for the sender's site code; nothing else is trusted.

use std::fmt;
use std::str::FromStr;

use aws_lc_rs::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;

/// Bytes of the key id: the first bytes of the key's SHA-256 fingerprint.
pub const KEY_ID: usize = 4;
/// Bytes of an Ed25519 signature.
pub const SIGNATURE: usize = 64;
/// What signing adds to a message.
pub const TRAILER: usize = KEY_ID + SIGNATURE;
/// How a public key is written: `ed25519:` then the 32 bytes in base64.
pub const PREFIX: &str = "ed25519:";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SignError {
    #[error("the message is too short to carry a signature")]
    Short,
    #[error("signed with key {got}, but the key pinned for the site is {pinned}")]
    WrongKey { got: String, pinned: String },
    #[error("the signature does not verify")]
    BadSignature,
    #[error("not a public key: give `ed25519:` and 32 bytes in base64")]
    BadKey,
    #[error("the signing key file is damaged")]
    BadPrivateKey,
}

/// A node's public key.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PublicKey([u8; 32]);

impl PublicKey {
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// SHA-256 of the raw key.
    pub fn fingerprint(&self) -> [u8; 32] {
        let d = aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA256, &self.0);
        d.as_ref().try_into().expect("SHA-256 is 32 bytes")
    }

    /// The fingerprint for people to compare: its first 16 bytes in hex,
    /// in groups of four characters.
    pub fn fingerprint_text(&self) -> String {
        let hex: Vec<String> = self.fingerprint()[..16]
            .chunks(2)
            .map(|c| format!("{:02x}{:02x}", c[0], c[1]))
            .collect();
        hex.join(" ")
    }

    /// The id carried on every message this key signs.
    pub fn key_id(&self) -> [u8; KEY_ID] {
        self.fingerprint()[..KEY_ID]
            .try_into()
            .expect("fingerprint is long enough")
    }

    /// Check a signed message and return it without its signature (the
    /// bytes [`crate::wire::Message::decode`] reads).
    pub fn verify<'a>(&self, signed: &'a [u8]) -> Result<&'a [u8], SignError> {
        let Parts {
            message,
            key_id,
            signature,
        } = split(signed)?;
        if key_id != self.key_id() {
            return Err(SignError::WrongKey {
                got: hex(&key_id),
                pinned: hex(&self.key_id()),
            });
        }
        UnparsedPublicKey::new(&ED25519, &self.0)
            .verify(&signed[..signed.len() - SIGNATURE], signature)
            .map_err(|_| SignError::BadSignature)?;
        Ok(message)
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{PREFIX}{}", B64.encode(self.0))
    }
}

impl fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

impl FromStr for PublicKey {
    type Err = SignError;

    /// `ed25519:<base64>`, or the base64 alone; spaces and line breaks
    /// (from copying) are ignored.
    fn from_str(s: &str) -> Result<Self, SignError> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        let b64 = s.strip_prefix(PREFIX).unwrap_or(&s);
        let bytes = B64
            .decode(b64)
            .or_else(|_| {
                base64::engine::general_purpose::STANDARD_NO_PAD.decode(b64.trim_end_matches('='))
            })
            .map_err(|_| SignError::BadKey)?;
        Ok(Self(bytes.try_into().map_err(|_| SignError::BadKey)?))
    }
}

impl serde::Serialize for PublicKey {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// A node's signing key.
pub struct NodeKey {
    pair: Ed25519KeyPair,
    public: PublicKey,
}

impl NodeKey {
    /// A new key, as the PKCS #8 document to keep.
    pub fn generate() -> Result<Vec<u8>, SignError> {
        let rng = aws_lc_rs::rand::SystemRandom::new();
        Ed25519KeyPair::generate_pkcs8v1(&rng)
            .map(|d| d.as_ref().to_vec())
            .map_err(|_| SignError::BadPrivateKey)
    }

    /// The key a PKCS #8 document holds.
    pub fn from_pkcs8(der: &[u8]) -> Result<Self, SignError> {
        let pair = Ed25519KeyPair::from_pkcs8(der).map_err(|_| SignError::BadPrivateKey)?;
        let public = PublicKey(
            pair.public_key()
                .as_ref()
                .try_into()
                .map_err(|_| SignError::BadPrivateKey)?,
        );
        Ok(Self { pair, public })
    }

    pub fn public(&self) -> PublicKey {
        self.public
    }

    /// Sign an encoded message: append the key id, then the signature over
    /// everything before it.
    pub fn sign(&self, mut message: Vec<u8>) -> Vec<u8> {
        message.extend_from_slice(&self.public.key_id());
        let sig = self.pair.sign(&message);
        message.extend_from_slice(sig.as_ref());
        message
    }
}

/// A signed message's parts.
pub struct Parts<'a> {
    /// The message as [`crate::wire::Message::decode`] reads it.
    pub message: &'a [u8],
    pub key_id: [u8; KEY_ID],
    pub signature: &'a [u8],
}

/// Split a signed message into its parts (nothing is checked).
pub fn split(signed: &[u8]) -> Result<Parts<'_>, SignError> {
    if signed.len() < crate::wire::HEADER + TRAILER {
        return Err(SignError::Short);
    }
    let (rest, signature) = signed.split_at(signed.len() - SIGNATURE);
    let (message, key_id) = rest.split_at(rest.len() - KEY_ID);
    Ok(Parts {
        message,
        key_id: key_id.try_into().expect("split at KEY_ID"),
        signature,
    })
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Hlc;
    use crate::wire::{Body, Message, Summary};

    fn key() -> NodeKey {
        NodeKey::from_pkcs8(&NodeKey::generate().unwrap()).unwrap()
    }

    fn message() -> Vec<u8> {
        Message::new(
            "AAA".parse().unwrap(),
            Hlc::new(1_790_000_000_000, 0),
            Body::Summary(Summary {
                heads: vec![("BBB".parse().unwrap(), 4)],
                reporting: 3,
            }),
        )
        .encode()
    }

    #[test]
    fn a_signed_message_verifies_and_reads_back() {
        let k = key();
        let plain = message();
        let signed = k.sign(plain.clone());
        assert_eq!(signed.len(), plain.len() + TRAILER);
        let back = k.public().verify(&signed).unwrap();
        assert_eq!(back, &plain[..]);
        assert!(Message::decode(back).is_ok());
    }

    #[test]
    fn a_changed_byte_anywhere_is_refused() {
        let k = key();
        let signed = k.sign(message());
        for i in 0..signed.len() {
            let mut v = signed.clone();
            v[i] ^= 1;
            assert!(k.public().verify(&v).is_err(), "byte {i}");
        }
    }

    #[test]
    fn another_key_is_refused() {
        let (a, b) = (key(), key());
        let signed = a.sign(message());
        assert!(matches!(
            b.public().verify(&signed),
            Err(SignError::WrongKey { .. })
        ));
        // Even with a's key id: the signature is what counts.
        let mut forged = b.sign(message());
        let n = forged.len();
        forged[n - TRAILER..n - SIGNATURE].copy_from_slice(&a.public().key_id());
        assert_eq!(a.public().verify(&forged), Err(SignError::BadSignature));
        assert_eq!(a.public().verify(&message()[..10]), Err(SignError::Short));
    }

    #[test]
    fn public_keys_are_written_and_read() {
        let p = key().public();
        let text = p.to_string();
        assert!(text.starts_with("ed25519:"));
        assert_eq!(text.parse::<PublicKey>().unwrap(), p);
        assert_eq!(text[8..].parse::<PublicKey>().unwrap(), p);
        let wrapped = format!(" {}\n{} ", &text[..20], &text[20..]);
        assert_eq!(wrapped.parse::<PublicKey>().unwrap(), p);
        assert!("ed25519:AAAA".parse::<PublicKey>().is_err());
        assert!("not a key".parse::<PublicKey>().is_err());
        assert_eq!(p.fingerprint_text().len(), 39);
        assert_eq!(&p.fingerprint()[..4], &p.key_id());
    }
}
