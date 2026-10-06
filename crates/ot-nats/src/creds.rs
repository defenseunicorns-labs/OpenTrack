//! NATS `.creds` sign-in with the FIPS module: the user JWT is sent as is and
//! the server's nonce is signed with the user's Ed25519 NKey by AWS-LC, not
//! by the `nkeys` crate async-nats would otherwise use.

use aws_lc_rs::signature::Ed25519KeyPair;
#[cfg(test)]
use aws_lc_rs::signature::KeyPair;

/// A user JWT and the key pair from its seed.
pub struct Creds {
    pub jwt: String,
    key: Ed25519KeyPair,
}

impl std::fmt::Debug for Creds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Creds").finish_non_exhaustive()
    }
}

impl Creds {
    pub fn parse(text: &str) -> Result<Self, String> {
        let jwt = section(text, "NATS USER JWT").ok_or("no user JWT in the file")?;
        let seed = section(text, "USER NKEY SEED").ok_or("no user NKey seed in the file")?;
        let raw = seed_bytes(&seed)?;
        let key = Ed25519KeyPair::from_seed_unchecked(&raw)
            .map_err(|_| "the NKey seed is not an Ed25519 key".to_owned())?;
        Ok(Self { jwt, key })
    }

    /// The signature over the server's nonce, raw (async-nats encodes it).
    pub fn sign(&self, nonce: &[u8]) -> Vec<u8> {
        self.key.sign(nonce).as_ref().to_vec()
    }

    #[cfg(test)]
    fn public_key(&self) -> Vec<u8> {
        self.key.public_key().as_ref().to_vec()
    }
}

/// The text between `-----BEGIN <name>-----` and the following `-----END`
/// line (NATS writes the END marker with six dashes; either is accepted).
fn section(text: &str, name: &str) -> Option<String> {
    let begin = format!("BEGIN {name}");
    let mut lines = text.lines().skip_while(|l| !l.contains(&begin));
    lines.next()?;
    let body: String = lines
        .take_while(|l| !l.contains("END "))
        .map(str::trim)
        .collect();
    (!body.is_empty()).then_some(body)
}

/// Seed prefixes (nkeys): "S" for a seed, "U" for a user.
const PREFIX_SEED: u8 = 18 << 3;
const PREFIX_USER: u8 = 20 << 3;

/// The 32-byte Ed25519 seed inside an encoded NKey user seed (`SU…`).
fn seed_bytes(seed: &str) -> Result<[u8; 32], String> {
    let raw = base32(seed.trim()).ok_or("the NKey seed is not base32")?;
    if raw.len() != 36 {
        return Err("the NKey seed has the wrong length".into());
    }
    let (body, crc) = raw.split_at(34);
    if crc16(body) != u16::from_le_bytes([crc[0], crc[1]]) {
        return Err("the NKey seed's checksum does not match".into());
    }
    let kind = ((body[0] & 0b0000_0111) << 5) | ((body[1] & 0b1111_1000) >> 3);
    if body[0] & 0b1111_1000 != PREFIX_SEED || kind != PREFIX_USER {
        return Err("not a user NKey seed".into());
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&body[2..]);
    Ok(out)
}

/// RFC 4648 base32 without padding.
fn base32(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 5 / 8);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'2'..=b'7' => c - b'2' + 26,
            b'=' => break,
            _ => return None,
        };
        acc = (acc << 5) | u32::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// CRC-16/XMODEM, the NKey checksum.
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= u16::from(b) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b32(bytes: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
        let (mut out, mut acc, mut bits) = (String::new(), 0u32, 0u32);
        for &b in bytes {
            acc = (acc << 8) | u32::from(b);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                out.push(A[((acc >> bits) & 31) as usize] as char);
            }
        }
        if bits > 0 {
            out.push(A[((acc << (5 - bits)) & 31) as usize] as char);
        }
        out
    }

    /// Encode a user seed the way nkeys does.
    fn encode_seed(seed: &[u8; 32]) -> String {
        let mut raw = vec![
            PREFIX_SEED | (PREFIX_USER >> 5),
            (PREFIX_USER & 0b0001_1111) << 3,
        ];
        raw.extend_from_slice(seed);
        let crc = crc16(&raw);
        raw.extend_from_slice(&crc.to_le_bytes());
        b32(&raw)
    }

    #[test]
    fn crc16_matches_xmodem() {
        assert_eq!(crc16(b"123456789"), 0x31C3);
    }

    #[test]
    fn a_creds_file_signs_with_its_seed() {
        let seed = [7u8; 32];
        let text = format!(
            "-----BEGIN NATS USER JWT-----\neyJhbGciOi.x.y\n------END NATS USER JWT------\n\n\
             ************************* IMPORTANT *************************\n\n\
             -----BEGIN USER NKEY SEED-----\n{}\n------END USER NKEY SEED------\n",
            encode_seed(&seed)
        );
        let c = Creds::parse(&text).unwrap();
        assert_eq!(c.jwt, "eyJhbGciOi.x.y");
        let sig = c.sign(b"nonce");
        let pk = aws_lc_rs::signature::UnparsedPublicKey::new(
            &aws_lc_rs::signature::ED25519,
            c.public_key(),
        );
        assert!(pk.verify(b"nonce", &sig).is_ok());
        let direct = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        assert_eq!(c.public_key(), direct.public_key().as_ref());
    }

    #[test]
    fn a_damaged_seed_is_refused() {
        let mut s = encode_seed(&[1u8; 32]);
        s.replace_range(10..11, if &s[10..11] == "A" { "B" } else { "A" });
        assert!(seed_bytes(&s).is_err());
        assert!(seed_bytes("not base32!").is_err());
    }
}
