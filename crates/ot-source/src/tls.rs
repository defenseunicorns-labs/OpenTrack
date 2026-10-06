//! TLS for feed transports: settings, and the rustls configurations built
//! from them.
//!
//! Client transports (`tcp_client`, `http_poll`, `websocket`, `mqtt`) share
//! [`ClientTls`]; `tcp_server` takes [`ServerTls`]. Certificates and keys are
//! PEM files, read (after `${env:NAME}` resolution) each time the transport
//! starts, so a renewed certificate is picked up on the next reconnect.

use std::sync::Arc;

use anyhow::{Context, bail};
use rustls::client::WebPkiServerVerifier;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{
    CertificateDer, CertificateRevocationListDer, PrivateKeyDer, ServerName, UnixTime,
};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::ClientCertVerifier;
use rustls::{DigitallySignedStruct, RootCertStore, SignatureScheme};
use serde::{Deserialize, Serialize};

use crate::transport::resolve_env;

/// TLS for a client transport.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientTls {
    /// PEM file of the CA(s) to trust instead of the system roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_file: Option<String>,
    /// PEM client certificate (chain) for mutual TLS; needs `key_file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_file: Option<String>,
    /// PEM private key for `cert_file`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_file: Option<String>,
    /// Verify the server's certificate against this name instead of the
    /// host being connected to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_name: Option<String>,
    /// Accept any server certificate. Development only.
    #[serde(default, skip_serializing_if = "is_false")]
    pub insecure_skip_verify: bool,
}

/// TLS for `tcp_server`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerTls {
    /// PEM server certificate (chain).
    pub cert_file: String,
    /// PEM private key for `cert_file`.
    pub key_file: String,
    /// PEM CA(s): when set, clients must present a certificate it signed
    /// (mutual TLS); otherwise any client may connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_ca_file: Option<String>,
    /// With `client_ca_file`: also let clients without a certificate
    /// connect (a certificate, when given, must still verify).
    #[serde(default, skip_serializing_if = "is_false")]
    pub client_cert_optional: bool,
    /// Certificate revocation lists for client certificates: files (PEM or
    /// DER) or directories of them. Given, a client certificate is refused
    /// when revoked, when no list covers its issuer, or when its issuer's
    /// list has expired (so keep them fresh; see [`ServerTls::crl_stamp`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub client_crl_files: Vec<String>,
}

fn is_false(b: &bool) -> bool {
    !b
}

fn set(v: &Option<String>) -> bool {
    v.as_deref().is_some_and(|s| !s.trim().is_empty())
}

impl ClientTls {
    /// Settings that cannot be caught by the type alone.
    pub fn check(&self) -> Result<(), String> {
        match (set(&self.cert_file), set(&self.key_file)) {
            (true, false) => return Err("tls.cert_file needs a tls.key_file".into()),
            (false, true) => return Err("tls.key_file needs a tls.cert_file".into()),
            _ => {}
        }
        if self.server_name.as_deref().is_some_and(|n| {
            !n.contains("${env:") && ServerName::try_from(n.trim().to_owned()).is_err()
        }) {
            return Err(format!(
                "tls.server_name {:?} is not a DNS name or IP address",
                self.server_name.as_deref().unwrap_or_default()
            ));
        }
        Ok(())
    }

    /// The `server_name` override, resolved.
    pub fn server_name(&self) -> anyhow::Result<Option<ServerName<'static>>> {
        match self.server_name.as_deref().filter(|n| !n.trim().is_empty()) {
            Some(n) => Ok(Some(server_name(&resolve_env(n)?)?)),
            None => Ok(None),
        }
    }

    /// A rustls client configuration for these settings.
    pub fn client_config(&self) -> anyhow::Result<rustls::ClientConfig> {
        self.check().map_err(anyhow::Error::msg)?;
        let provider = provider();
        let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()?;
        let builder = if self.insecure_skip_verify {
            tracing::warn!(
                "TLS server certificate verification is OFF (tls.insecure_skip_verify): \
                 anyone on the path can impersonate this feed. Development only!"
            );
            builder
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(AcceptAny(provider)))
        } else {
            let roots = match self.ca_file.as_deref().filter(|p| !p.trim().is_empty()) {
                Some(path) => roots("tls.ca_file", path)?,
                None => system_roots()?,
            };
            let verifier =
                WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider).build()?;
            match self.server_name()? {
                Some(name) => builder
                    .dangerous()
                    .with_custom_certificate_verifier(Arc::new(VerifyAs { verifier, name })),
                None => builder.with_webpki_verifier(verifier),
            }
        };
        match (&self.cert_file, &self.key_file) {
            (Some(cert), Some(key)) if set(&self.cert_file) => {
                let chain = certs("tls.cert_file", cert)?;
                let key = private_key("tls.key_file", key)?;
                builder
                    .with_client_auth_cert(chain, key)
                    .context("tls.cert_file / tls.key_file: the key does not fit the certificate")
            }
            _ => Ok(builder.with_no_client_auth()),
        }
    }
}

impl ServerTls {
    /// Settings that cannot be caught by the type alone.
    pub fn check(&self) -> Result<(), String> {
        if self.cert_file.trim().is_empty() {
            return Err("tls.cert_file is required for a TLS server".into());
        }
        if self.key_file.trim().is_empty() {
            return Err("tls.key_file is required for a TLS server".into());
        }
        Ok(())
    }

    /// A TLS acceptor for these settings.
    pub fn acceptor(&self) -> anyhow::Result<tokio_rustls::TlsAcceptor> {
        Ok(tokio_rustls::TlsAcceptor::from(Arc::new(
            self.server_config()?,
        )))
    }

    /// The same, offering HTTP/2 (ALPN `h2`), which gRPC clients require.
    pub fn h2_acceptor(&self) -> anyhow::Result<tokio_rustls::TlsAcceptor> {
        let mut config = self.server_config()?;
        config.alpn_protocols = vec![b"h2".to_vec()];
        Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
    }

    fn server_config(&self) -> anyhow::Result<rustls::ServerConfig> {
        self.check().map_err(anyhow::Error::msg)?;
        let provider = provider();
        let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()?;
        let builder = match self
            .client_ca_file
            .as_deref()
            .filter(|p| !p.trim().is_empty())
        {
            Some(path) => builder.with_client_cert_verifier(self.client_verifier(
                path,
                provider,
                crls(&self.client_crl_files)?,
            )?),
            None => builder.with_no_client_auth(),
        };
        let config = builder
            .with_single_cert(
                certs("tls.cert_file", &self.cert_file)?,
                private_key("tls.key_file", &self.key_file)?,
            )
            .context("tls.cert_file / tls.key_file: the key does not fit the certificate")?;
        Ok(config)
    }
}

impl ServerTls {
    fn client_verifier(
        &self,
        ca_path: &str,
        provider: Arc<CryptoProvider>,
        crls: Vec<CertificateRevocationListDer<'static>>,
    ) -> anyhow::Result<Arc<dyn ClientCertVerifier>> {
        let verifier = WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots("tls.client_ca_file", ca_path)?),
            provider,
        );
        let verifier = if self.client_cert_optional {
            verifier.allow_unauthenticated()
        } else {
            verifier
        };
        let verifier = if crls.is_empty() {
            verifier
        } else {
            // The end certificate's status must be known and its
            // issuer's list current: fail closed.
            verifier
                .with_crls(crls)
                .only_check_end_entity_revocation()
                .enforce_revocation_expiration()
        };
        Ok(verifier.build()?)
    }

    /// The client certificate verifier with the revocation lists, as the
    /// handshake uses it: for a status check made after the handshake (the
    /// control plane asks OCSP first and falls back to this). `None`
    /// without a client CA or without lists.
    pub fn client_crl_verifier(&self) -> anyhow::Result<Option<Arc<dyn ClientCertVerifier>>> {
        let Some(path) = self
            .client_ca_file
            .as_deref()
            .filter(|p| !p.trim().is_empty())
        else {
            return Ok(None);
        };
        let crls = crls(&self.client_crl_files)?;
        if crls.is_empty() {
            return Ok(None);
        }
        self.client_verifier(path, provider(), crls).map(Some)
    }

    /// The client CA certificates (`client_ca_file`); none without one.
    pub fn client_ca_certs(&self) -> anyhow::Result<Vec<CertificateDer<'static>>> {
        match self
            .client_ca_file
            .as_deref()
            .filter(|p| !p.trim().is_empty())
        {
            Some(path) => certs("tls.client_ca_file", path),
            None => Ok(Vec::new()),
        }
    }
}

/// The process's crypto provider (installed by the server), else AWS-LC in
/// its FIPS configuration.
fn provider() -> Arc<CryptoProvider> {
    CryptoProvider::get_default()
        .cloned()
        .unwrap_or_else(|| Arc::new(rustls::crypto::default_fips_provider()))
}

pub(crate) fn server_name(name: &str) -> anyhow::Result<ServerName<'static>> {
    // IPv6 hosts may arrive bracketed, as in URLs.
    let name = name.trim().trim_start_matches('[').trim_end_matches(']');
    ServerName::try_from(name.to_owned())
        .with_context(|| format!("{name:?} is not a valid TLS server name"))
}

fn read(field: &str, path: &str) -> anyhow::Result<(String, Vec<u8>)> {
    let path = resolve_env(path).context(field.to_owned())?;
    let bytes = std::fs::read(&path).with_context(|| format!("{field} {path}"))?;
    Ok((path, bytes))
}

/// The CRL files named, a directory standing for the files in it.
fn crl_paths(paths: &[String]) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    for p in paths.iter().filter(|p| !p.trim().is_empty()) {
        let p = std::path::PathBuf::from(resolve_env(p).context("tls.client_crl_files")?);
        if p.is_dir() {
            let mut files: Vec<_> = std::fs::read_dir(&p)
                .with_context(|| format!("tls.client_crl_files {}", p.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|f| f.is_file())
                .collect();
            files.sort();
            out.extend(files);
        } else {
            out.push(p);
        }
    }
    Ok(out)
}

/// Every CRL in the files (PEM, with any number of lists, or one DER list).
fn crls(paths: &[String]) -> anyhow::Result<Vec<CertificateRevocationListDer<'static>>> {
    let mut out = Vec::new();
    for path in crl_paths(paths)? {
        let bytes = std::fs::read(&path)
            .with_context(|| format!("tls.client_crl_files {}", path.display()))?;
        let pem = CertificateRevocationListDer::pem_slice_iter(&bytes)
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default();
        if pem.is_empty() {
            out.push(CertificateRevocationListDer::from(bytes));
        } else {
            out.extend(pem);
        }
    }
    Ok(out)
}

impl ServerTls {
    /// What the CRL files look like now (their names, sizes and times): a
    /// change means the acceptor should be rebuilt to pick them up.
    pub fn crl_stamp(&self) -> String {
        crl_paths(&self.client_crl_files)
            .unwrap_or_default()
            .iter()
            .map(|p| {
                let m = std::fs::metadata(p).ok();
                format!(
                    "{}:{}:{:?};",
                    p.display(),
                    m.as_ref().map_or(0, std::fs::Metadata::len),
                    m.and_then(|m| m.modified().ok())
                )
            })
            .collect()
    }
}

fn certs(field: &str, path: &str) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let (path, pem) = read(field, path)?;
    let certs = CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("{field} {path}: {e}"))?;
    if certs.is_empty() {
        bail!("{field} {path}: no certificates found");
    }
    Ok(certs)
}

fn private_key(field: &str, path: &str) -> anyhow::Result<PrivateKeyDer<'static>> {
    let (path, pem) = read(field, path)?;
    PrivateKeyDer::from_pem_slice(&pem).map_err(|e| match e {
        rustls::pki_types::pem::Error::NoItemsFound => {
            anyhow::anyhow!("{field} {path}: no private key found")
        }
        e => anyhow::anyhow!("{field} {path}: {e}"),
    })
}

fn roots(field: &str, path: &str) -> anyhow::Result<RootCertStore> {
    let mut roots = RootCertStore::empty();
    for cert in certs(field, path)? {
        roots
            .add(cert)
            .map_err(|e| anyhow::anyhow!("{field} {path}: {e}"))?;
    }
    Ok(roots)
}

fn system_roots() -> anyhow::Result<RootCertStore> {
    let found = rustls_native_certs::load_native_certs();
    for e in &found.errors {
        tracing::debug!(error = %e, "loading a system root certificate");
    }
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(found.certs);
    if roots.is_empty() {
        bail!("no system root certificates found; set tls.ca_file");
    }
    Ok(roots)
}

/// Verifies against a fixed name, whatever host was dialled (`server_name`).
#[derive(Debug)]
struct VerifyAs {
    verifier: Arc<WebPkiServerVerifier>,
    name: ServerName<'static>,
}

impl ServerCertVerifier for VerifyAs {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        self.verifier
            .verify_server_cert(end_entity, intermediates, &self.name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verifier.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        self.verifier.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.verifier.supported_verify_schemes()
    }
}

/// `insecure_skip_verify`: any certificate, though the handshake signatures
/// are still checked (the peer must hold the key it presents).
#[derive(Debug)]
struct AcceptAny(Arc<CryptoProvider>);

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// A certificate subject's common name.
pub fn common_name(cert: &[u8]) -> Option<String> {
    subject(cert)?
        .split(", ")
        .find_map(|p| p.strip_prefix("CN="))
        .map(str::to_owned)
}

/// A certificate's subject as `CN=…, O=…`, for logs. Reads just enough DER
/// to find it; `None` if the certificate is not as expected.
pub fn subject(cert: &[u8]) -> Option<String> {
    let (_, cert, _) = der(cert)?;
    let (_, tbs, _) = der(cert)?;
    // Optional [0] version, then serial, signature, issuer, validity.
    let mut rest = tbs;
    if der(rest)?.0 == 0xa0 {
        rest = der(rest)?.2;
    }
    for _ in 0..4 {
        rest = der(rest)?.2;
    }
    let (_, mut name, _) = der(rest)?;
    let mut parts = Vec::new();
    while !name.is_empty() {
        let (_, mut rdn, next) = der(name)?;
        name = next;
        while !rdn.is_empty() {
            let (_, atv, next) = der(rdn)?;
            rdn = next;
            let (_, oid, value) = der(atv)?;
            let (_, value, _) = der(value)?;
            let key = match oid {
                [0x55, 0x04, 0x03] => "CN".to_owned(),
                [0x55, 0x04, 0x06] => "C".to_owned(),
                [0x55, 0x04, 0x07] => "L".to_owned(),
                [0x55, 0x04, 0x08] => "ST".to_owned(),
                [0x55, 0x04, 0x0a] => "O".to_owned(),
                [0x55, 0x04, 0x0b] => "OU".to_owned(),
                other => format!("oid:{}", hex(other)),
            };
            parts.push(format!("{key}={}", String::from_utf8_lossy(value)));
        }
    }
    Some(parts.join(", "))
}

/// One DER element: (tag, contents, rest).
fn der(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, input) = input.split_first()?;
    let (&first, mut input) = input.split_first()?;
    let len = if first < 0x80 {
        usize::from(first)
    } else {
        let n = usize::from(first & 0x7f);
        if n == 0 || n > 4 || input.len() < n {
            return None;
        }
        let len = input[..n]
            .iter()
            .fold(0usize, |acc, b| (acc << 8) | usize::from(*b));
        input = &input[n..];
        len
    };
    (input.len() >= len).then(|| (tag, &input[..len], &input[len..]))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
