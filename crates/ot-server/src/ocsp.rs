//! OCSP (RFC 6960) for client certificates: just enough DER to read a
//! certificate, ask a responder about it and check the answer.
//!
//! No ASN.1 crate is in the build, so this reads and writes the few
//! structures it needs by hand, strictly: definite lengths only, minimal
//! length encodings, every element bounds-checked, nothing left over.
//! Signatures are checked by `rustls-webpki` with the process's FIPS
//! provider's algorithms (AWS-LC). The CertID hashes are SHA-1, as RFC 6960
//! defines them; that is an identifier, not a signature (see
//! docs/security/fips.md).

use rustls::pki_types::{CertificateDer, SignatureVerificationAlgorithm, UnixTime};

/// Why a certificate or answer could not be used.
pub type Result<T> = std::result::Result<T, &'static str>;

pub const SEQUENCE: u8 = 0x30;
pub const INTEGER: u8 = 0x02;
pub const BIT_STRING: u8 = 0x03;
pub const OCTET_STRING: u8 = 0x04;
pub const NULL: u8 = 0x05;
pub const OID: u8 = 0x06;
const BOOLEAN: u8 = 0x01;
const ENUMERATED: u8 = 0x0a;
const GENERALIZED_TIME: u8 = 0x18;

/// 1.3.14.3.2.26, SHA-1 (CertID only).
const OID_SHA1: &[u8] = &[0x2b, 0x0e, 0x03, 0x02, 0x1a];
/// 1.3.6.1.5.5.7.1.1, authority information access.
const OID_AIA: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x01];
/// 1.3.6.1.5.5.7.48.1, id-ad-ocsp.
const OID_AD_OCSP: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01];
/// 1.3.6.1.5.5.7.48.1.1, id-pkix-ocsp-basic.
const OID_OCSP_BASIC: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01, 0x01];
/// 1.3.6.1.5.5.7.48.1.2, id-pkix-ocsp-nonce.
const OID_OCSP_NONCE: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01, 0x02];
/// 1.3.6.1.5.5.7.3.9, id-kp-OCSPSigning.
const OID_OCSP_SIGNING: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x09];

/// A strict DER reader over a run of elements.
#[derive(Clone, Copy)]
pub struct Der<'a>(&'a [u8]);

impl<'a> Der<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self(input)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn peek(&self) -> Option<u8> {
        self.0.first().copied()
    }

    /// The next element: (tag, contents, the whole element).
    pub fn next(&mut self) -> Result<(u8, &'a [u8], &'a [u8])> {
        let input = self.0;
        let (&tag, rest) = input.split_first().ok_or("truncated DER")?;
        if tag & 0x1f == 0x1f {
            return Err("unsupported DER tag");
        }
        let (&first, mut rest) = rest.split_first().ok_or("truncated DER")?;
        let len = if first < 0x80 {
            usize::from(first)
        } else {
            let n = usize::from(first & 0x7f);
            // 0x80 is BER's indefinite length; more than four bytes is
            // more than any certificate or answer needs.
            if n == 0 || n > 4 || rest.len() < n {
                return Err("bad DER length");
            }
            if rest[0] == 0 {
                return Err("non-minimal DER length");
            }
            let len = rest[..n]
                .iter()
                .fold(0usize, |acc, b| (acc << 8) | usize::from(*b));
            if len < 0x80 {
                return Err("non-minimal DER length");
            }
            rest = &rest[n..];
            len
        };
        if rest.len() < len {
            return Err("truncated DER");
        }
        let header = input.len() - rest.len();
        self.0 = &rest[len..];
        Ok((tag, &rest[..len], &input[..header + len]))
    }

    /// The contents of the next element, which must have `tag`.
    pub fn expect(&mut self, tag: u8) -> Result<&'a [u8]> {
        match self.next()? {
            (t, contents, _) if t == tag => Ok(contents),
            _ => Err("unexpected DER element"),
        }
    }

    /// The contents of the next element if it has `tag`.
    pub fn optional(&mut self, tag: u8) -> Result<Option<&'a [u8]>> {
        if self.peek() == Some(tag) {
            self.expect(tag).map(Some)
        } else {
            Ok(None)
        }
    }

    /// Nothing may follow.
    pub fn end(&self) -> Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err("trailing data in DER")
        }
    }
}

/// The contents of `input`, which must be exactly one element with `tag`.
pub fn one(input: &[u8], tag: u8) -> Result<&[u8]> {
    let mut d = Der::new(input);
    let contents = d.expect(tag)?;
    d.end()?;
    Ok(contents)
}

/// One DER element with these contents.
pub fn tlv(tag: u8, parts: &[&[u8]]) -> Vec<u8> {
    let len: usize = parts.iter().map(|p| p.len()).sum();
    let mut out = vec![tag];
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes = len.to_be_bytes();
        let skip = bytes.iter().take_while(|b| **b == 0).count();
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    for p in parts {
        out.extend_from_slice(p);
    }
    out
}

/// A BIT STRING's bits, which must be whole bytes.
fn bits(contents: &[u8]) -> Result<&[u8]> {
    match contents.split_first() {
        Some((0, rest)) => Ok(rest),
        _ => Err("BIT STRING with unused bits"),
    }
}

/// The parts of an X.509 certificate OCSP needs.
pub struct Cert<'a> {
    /// The whole TBSCertificate element (what the signature covers).
    pub tbs: &'a [u8],
    /// The signatureAlgorithm's contents.
    pub sig_alg: &'a [u8],
    pub signature: &'a [u8],
    /// The serial number INTEGER's contents.
    pub serial: &'a [u8],
    /// The issuer and subject Name elements, whole.
    pub issuer: &'a [u8],
    pub subject: &'a [u8],
    /// The subjectPublicKey BIT STRING's bits (what issuerKeyHash hashes).
    pub public_key: &'a [u8],
    /// OCSP responder URLs from the authority information access extension.
    pub ocsp_urls: Vec<&'a str>,
}

impl<'a> Cert<'a> {
    pub fn parse(der: &'a [u8]) -> Result<Self> {
        let mut cert = Der::new(one(der, SEQUENCE)?);
        let (tag, tbs_contents, tbs) = cert.next()?;
        if tag != SEQUENCE {
            return Err("not a certificate");
        }
        let sig_alg = cert.expect(SEQUENCE)?;
        let signature = bits(cert.expect(BIT_STRING)?)?;
        cert.end()?;

        let mut t = Der::new(tbs_contents);
        t.optional(0xa0)?;
        let serial = t.expect(INTEGER)?;
        t.expect(SEQUENCE)?;
        let (tag, _, issuer) = t.next()?;
        if tag != SEQUENCE {
            return Err("certificate issuer is not a Name");
        }
        t.expect(SEQUENCE)?;
        let (tag, _, subject) = t.next()?;
        if tag != SEQUENCE {
            return Err("certificate subject is not a Name");
        }
        let mut spki = Der::new(t.expect(SEQUENCE)?);
        spki.expect(SEQUENCE)?;
        let public_key = bits(spki.expect(BIT_STRING)?)?;
        spki.end()?;
        t.optional(0x81)?;
        t.optional(0x82)?;
        let mut ocsp_urls = Vec::new();
        if let Some(exts) = t.optional(0xa3)? {
            let mut exts = Der::new(one(exts, SEQUENCE)?);
            while !exts.is_empty() {
                let (id, _, value) = extension(exts.expect(SEQUENCE)?)?;
                if id == OID_AIA {
                    ocsp_urls = aia_ocsp(value)?;
                }
            }
        }
        t.end()?;
        Ok(Self {
            tbs,
            sig_alg,
            signature,
            serial,
            issuer,
            subject,
            public_key,
            ocsp_urls,
        })
    }
}

/// An Extension's (extnID, critical, extnValue contents).
fn extension(contents: &[u8]) -> Result<(&[u8], bool, &[u8])> {
    let mut e = Der::new(contents);
    let id = e.expect(OID)?;
    let critical = match e.optional(BOOLEAN)? {
        Some([0xff]) => true,
        Some([0x00]) => false,
        Some(_) => return Err("bad BOOLEAN"),
        None => false,
    };
    let value = e.expect(OCTET_STRING)?;
    e.end()?;
    Ok((id, critical, value))
}

/// The id-ad-ocsp URIs in an AuthorityInfoAccessSyntax.
fn aia_ocsp(value: &[u8]) -> Result<Vec<&str>> {
    let mut out = Vec::new();
    let mut list = Der::new(one(value, SEQUENCE)?);
    while !list.is_empty() {
        let mut ad = Der::new(list.expect(SEQUENCE)?);
        let method = ad.expect(OID)?;
        let (tag, location, _) = ad.next()?;
        ad.end()?;
        // uniformResourceIdentifier [6] IMPLICIT IA5String.
        if method == OID_AD_OCSP && tag == 0x86 {
            let url = std::str::from_utf8(location).map_err(|_| "AIA URL is not text")?;
            if url.is_ascii() {
                out.push(url);
            }
        }
    }
    Ok(out)
}

/// Whether `signer_der`'s key made `signature` over `msg` with the
/// algorithm `alg_id` names (one the provider supports).
pub fn signed_by(
    algs: &[&dyn SignatureVerificationAlgorithm],
    signer_der: &[u8],
    alg_id: &[u8],
    msg: &[u8],
    signature: &[u8],
) -> bool {
    let der = CertificateDer::from(signer_der);
    let Ok(signer) = webpki::EndEntityCert::try_from(&der) else {
        return false;
    };
    algs.iter()
        .filter(|a| a.signature_alg_id().as_ref() == alg_id)
        .any(|a| signer.verify_signature(*a, msg, signature).is_ok())
}

/// Which certificate an OCSP request asks about (RFC 6960 4.1.1), with
/// SHA-1 hashes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertId {
    name_hash: Vec<u8>,
    key_hash: Vec<u8>,
    serial: Vec<u8>,
}

impl CertId {
    /// `leaf`, as issued by `issuer`.
    pub fn new(issuer: &Cert<'_>, leaf: &Cert<'_>) -> Self {
        let sha1 = |b: &[u8]| {
            aws_lc_rs::digest::digest(&aws_lc_rs::digest::SHA1_FOR_LEGACY_USE_ONLY, b)
                .as_ref()
                .to_vec()
        };
        Self {
            name_hash: sha1(issuer.subject),
            key_hash: sha1(issuer.public_key),
            serial: leaf.serial.to_vec(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        tlv(
            SEQUENCE,
            &[
                &sha1_alg(),
                &tlv(OCTET_STRING, &[&self.name_hash]),
                &tlv(OCTET_STRING, &[&self.key_hash]),
                &tlv(INTEGER, &[&self.serial]),
            ],
        )
    }

    /// Whether a response's CertID (contents) names this certificate.
    fn matches(&self, contents: &[u8]) -> Result<bool> {
        let mut c = Der::new(contents);
        let mut alg = Der::new(c.expect(SEQUENCE)?);
        let oid = alg.expect(OID)?;
        alg.optional(NULL)?;
        alg.end()?;
        let name = c.expect(OCTET_STRING)?;
        let key = c.expect(OCTET_STRING)?;
        let serial = c.expect(INTEGER)?;
        c.end()?;
        Ok(oid == OID_SHA1
            && name == self.name_hash
            && key == self.key_hash
            && serial == self.serial)
    }
}

fn sha1_alg() -> Vec<u8> {
    tlv(SEQUENCE, &[&tlv(OID, &[OID_SHA1]), &[NULL, 0]])
}

/// An OCSPRequest for one certificate with a nonce extension.
pub fn request(id: &CertId, nonce: &[u8]) -> Vec<u8> {
    let request_list = tlv(SEQUENCE, &[&tlv(SEQUENCE, &[&id.encode()])]);
    let nonce_ext = tlv(
        SEQUENCE,
        &[
            &tlv(OID, &[OID_OCSP_NONCE]),
            &tlv(OCTET_STRING, &[&tlv(OCTET_STRING, &[nonce])]),
        ],
    );
    let extensions = tlv(0xa2, &[&tlv(SEQUENCE, &[&nonce_ext])]);
    tlv(SEQUENCE, &[&tlv(SEQUENCE, &[&request_list, &extensions])])
}

/// What the responder said about the certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Good,
    Revoked,
    /// The responder doesn't know the certificate: not a revocation, but
    /// no vouching either.
    Unknown,
}

/// A checked answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Answer {
    pub status: Status,
    /// Unix seconds.
    pub this_update: i64,
    pub next_update: Option<i64>,
}

/// Clock skew allowed between us and the responder.
pub const SKEW_SECS: i64 = 300;
/// An answer without a nextUpdate is used only if made within the hour: it
/// limits replaying an old answer (a nonce is optional, and OCSP usually
/// travels over plain HTTP).
pub const NO_NEXT_UPDATE_MAX_AGE_SECS: i64 = 3600;

/// Everything a response is checked against.
pub struct Expect<'a> {
    pub id: &'a CertId,
    /// The certificate's issuer, DER.
    pub issuer: &'a [u8],
    pub nonce: &'a [u8],
    pub algs: &'a [&'a dyn SignatureVerificationAlgorithm],
    pub now: UnixTime,
}

/// Check an OCSPResponse: successful, a basic response signed by the
/// issuer or a responder it delegated to, a nonce (if any) that is ours,
/// an answer for our certificate, and current.
pub fn check_response(body: &[u8], x: &Expect<'_>) -> Result<Answer> {
    let mut outer = Der::new(one(body, SEQUENCE)?);
    match outer.expect(ENUMERATED)? {
        [0] => {}
        [1] => return Err("the responder answered malformedRequest"),
        [2] => return Err("the responder answered internalError"),
        [3] => return Err("the responder answered tryLater"),
        [5] => return Err("the responder answered sigRequired"),
        [6] => return Err("the responder answered unauthorized"),
        _ => return Err("the responder answered an unknown status"),
    }
    let bytes = outer.expect(0xa0)?;
    outer.end()?;
    let mut rb = Der::new(one(bytes, SEQUENCE)?);
    if rb.expect(OID)? != OID_OCSP_BASIC {
        return Err("not a basic OCSP response");
    }
    let basic = rb.expect(OCTET_STRING)?;
    rb.end()?;

    let mut basic = Der::new(one(basic, SEQUENCE)?);
    let (tag, data, tbs) = basic.next()?;
    if tag != SEQUENCE {
        return Err("bad ResponseData");
    }
    let sig_alg = basic.expect(SEQUENCE)?;
    let signature = bits(basic.expect(BIT_STRING)?)?;
    let mut certs = Vec::new();
    if let Some(c) = basic.optional(0xa0)? {
        let mut list = Der::new(one(c, SEQUENCE)?);
        while !list.is_empty() {
            let (tag, _, whole) = list.next()?;
            if tag != SEQUENCE {
                return Err("bad certificate in the response");
            }
            certs.push(whole);
        }
    }
    basic.end()?;

    // The signature first: nothing in the data counts until it verifies.
    if !signed_by(x.algs, x.issuer, sig_alg, tbs, signature)
        && !certs.iter().any(|c| {
            delegated(c, x.issuer, x.algs, x.now) && signed_by(x.algs, c, sig_alg, tbs, signature)
        })
    {
        return Err("the response's signature is not the CA's or a responder's it delegated to");
    }

    let mut d = Der::new(data);
    if let Some(v) = d.optional(0xa0)?
        && one(v, INTEGER)? != [0]
    {
        return Err("unsupported OCSP response version");
    }
    match d.next()?.0 {
        0xa1 | 0xa2 => {}
        _ => return Err("bad responderID"),
    }
    d.expect(GENERALIZED_TIME)?;
    let mut responses = Der::new(d.expect(SEQUENCE)?);
    if let Some(exts) = d.optional(0xa1)? {
        let mut exts = Der::new(one(exts, SEQUENCE)?);
        while !exts.is_empty() {
            let (id, critical, value) = extension(exts.expect(SEQUENCE)?)?;
            if id == OID_OCSP_NONCE {
                // RFC 8954 wraps the nonce in an OCTET STRING; older
                // responders sent it bare. Either way it must be ours.
                if one(value, OCTET_STRING).ok() != Some(x.nonce) && value != x.nonce {
                    return Err("the response's nonce is not ours");
                }
            } else if critical {
                return Err("unsupported critical response extension");
            }
        }
    }
    d.end()?;

    let now = i64::try_from(x.now.as_secs()).unwrap_or(i64::MAX);
    while !responses.is_empty() {
        let mut single = Der::new(responses.expect(SEQUENCE)?);
        if !x.id.matches(single.expect(SEQUENCE)?)? {
            continue;
        }
        let status = match single.next()? {
            (0x80, [], _) => Status::Good,
            (0xa1, _, _) => Status::Revoked,
            (0x82, [], _) => Status::Unknown,
            _ => return Err("bad certStatus"),
        };
        let this_update = time(single.expect(GENERALIZED_TIME)?)?;
        let next_update = match single.optional(0xa0)? {
            Some(t) => Some(time(one(t, GENERALIZED_TIME)?)?),
            None => None,
        };
        if let Some(exts) = single.optional(0xa1)? {
            let mut exts = Der::new(one(exts, SEQUENCE)?);
            while !exts.is_empty() {
                if extension(exts.expect(SEQUENCE)?)?.1 {
                    return Err("unsupported critical single extension");
                }
            }
        }
        single.end()?;
        if this_update > now + SKEW_SECS {
            return Err("the answer's thisUpdate is in the future");
        }
        match next_update {
            Some(n) if n < now - SKEW_SECS => return Err("the answer is past its nextUpdate"),
            None if this_update < now - NO_NEXT_UPDATE_MAX_AGE_SECS - SKEW_SECS => {
                return Err("the answer has no nextUpdate and is over an hour old");
            }
            _ => {}
        }
        return Ok(Answer {
            status,
            this_update,
            next_update,
        });
    }
    Err("the response has no answer for this certificate (CertID mismatch)")
}

/// Whether `responder` is a delegated OCSP responder: issued directly by
/// `issuer` (the sole trust anchor), valid now, with id-kp-OCSPSigning.
fn delegated(
    responder: &[u8],
    issuer: &[u8],
    algs: &[&dyn SignatureVerificationAlgorithm],
    now: UnixTime,
) -> bool {
    let (responder, issuer) = (
        CertificateDer::from(responder),
        CertificateDer::from(issuer),
    );
    let (Ok(ee), Ok(anchor)) = (
        webpki::EndEntityCert::try_from(&responder),
        webpki::anchor_from_trusted_cert(&issuer),
    ) else {
        return false;
    };
    ee.verify_for_usage(
        algs,
        &[anchor],
        &[],
        now,
        webpki::KeyUsage::required(OID_OCSP_SIGNING),
        None,
        None,
    )
    .is_ok()
}

/// A GeneralizedTime (`YYYYMMDDHHMMSS[.f]Z`) as Unix seconds.
fn time(t: &[u8]) -> Result<i64> {
    const BAD: &str = "bad GeneralizedTime";
    let t = std::str::from_utf8(t).map_err(|_| BAD)?;
    let t = t.strip_suffix('Z').ok_or(BAD)?;
    let (whole, frac) = t.split_once('.').unwrap_or((t, "0"));
    if whole.len() != 14
        || !whole.bytes().all(|b| b.is_ascii_digit())
        || frac.is_empty()
        || !frac.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(BAD);
    }
    let n = |r: std::ops::Range<usize>| whole[r].parse::<u32>().map_err(|_| BAD);
    let year = i32::try_from(n(0..4)?).map_err(|_| BAD)?;
    let (h, m, sec) = (n(8..10)?, n(10..12)?, n(12..14)?);
    chrono::NaiveDate::from_ymd_opt(year, n(4..6)?, n(6..8)?)
        .and_then(|d| d.and_hms_opt(h, m, sec))
        .map(|dt| dt.and_utc().timestamp())
        .ok_or(BAD)
}

/// The nonce of an OCSPRequest (for tests' responders).
#[cfg(test)]
pub fn request_nonce(req: &[u8]) -> Option<Vec<u8>> {
    let mut tbs = Der::new(one(one(req, SEQUENCE).ok()?, SEQUENCE).ok()?);
    tbs.expect(SEQUENCE).ok()?;
    let exts = tbs.optional(0xa2).ok()??;
    let mut exts = Der::new(one(exts, SEQUENCE).ok()?);
    while !exts.is_empty() {
        let (id, _, value) = extension(exts.expect(SEQUENCE).ok()?).ok()?;
        if id == OID_OCSP_NONCE {
            return Some(one(value, OCTET_STRING).ok()?.to_vec());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn der_lengths_are_read_strictly() {
        assert_eq!(Der::new(&[0x04, 0x01, 0xaa]).next().unwrap().1, [0xaa]);
        let long = tlv(OCTET_STRING, &[&[7u8; 300]]);
        assert_eq!(&long[..4], &[0x04, 0x82, 0x01, 0x2c]);
        assert_eq!(Der::new(&long).next().unwrap().1.len(), 300);
        // Indefinite, non-minimal, truncated, high tag number.
        assert!(Der::new(&[0x30, 0x80, 0, 0]).next().is_err());
        assert!(Der::new(&[0x04, 0x81, 0x05, 1, 2, 3, 4, 5]).next().is_err());
        assert!(Der::new(&[0x04, 0x82, 0x00, 0x90]).next().is_err());
        assert!(Der::new(&[0x04, 0x05, 1, 2]).next().is_err());
        assert!(Der::new(&[0x1f, 0x01, 0x00]).next().is_err());
        assert!(one(&[0x04, 0x00, 0x05, 0x00], OCTET_STRING).is_err());
    }

    #[test]
    fn generalized_times_parse() {
        assert_eq!(time(b"19700101000000Z"), Ok(0));
        assert_eq!(time(b"20260930120000.5Z"), Ok(1_790_769_600));
        assert!(time(b"20260930120000").is_err());
        assert!(time(b"2026093012000Z").is_err());
        assert!(time(b"20261301120000Z").is_err());
    }

    #[test]
    fn a_request_carries_the_cert_id_and_nonce() {
        let id = CertId {
            name_hash: vec![1; 20],
            key_hash: vec![2; 20],
            serial: vec![0x0b, 0xad],
        };
        let req = request(&id, &[9; 16]);
        assert_eq!(request_nonce(&req), Some(vec![9; 16]));
        let mut tbs = Der::new(one(one(&req, SEQUENCE).unwrap(), SEQUENCE).unwrap());
        let list = tbs.expect(SEQUENCE).unwrap();
        let request = one(list, SEQUENCE).unwrap();
        assert!(id.matches(one(request, SEQUENCE).unwrap()).unwrap());
    }
}
