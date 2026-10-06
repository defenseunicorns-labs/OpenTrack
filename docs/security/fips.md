# FIPS 140-3

From 0.4.0, OpenTrack uses only FIPS-validated cryptography, in every build.
No build or setting turns it off.

## The two modules

| Module | Used for | Where it comes from |
|---|---|---|
| **AWS-LC FIPS 3.0.x** (static, via `aws-lc-fips-sys` 0.14.2 / `aws-lc-rs` 1.x) | All of the binary's own cryptography: TLS for the UI/API, feeds, gRPC, MQTT, WebSocket, NATS, HTTP clients and OpenTelemetry export; session and API token signatures (HS256); password hashing; the NATS `.creds` nonce signature (Ed25519); sync message signatures (Ed25519) and key fingerprints (SHA-256); SHA-256 of plugin files; every random secret and identifier | Compiled into the binary from the module source that `aws-lc-fips-sys` carries. The build needs Go and CMake. |
| **OpenSSL 3.0.9 FIPS provider** (CMVP #4282) | SAML: libxmlsec1 checks the identity provider's XML signatures through OpenSSL | Built in the image from the 3.0.9 release as its security policy directs (`./Configure enable-fips`, `make install_fips`). The image's OpenSSL 3.0 library (Debian 12's `libssl3`, part of the distroless runtime base) loads it, and `default_properties = fips=yes` (`docker/openssl-fips.cnf`) leaves nothing else to use. |

Check each module's current CMVP status and security policy before you cite it
in an accreditation package: the [CMVP search](https://csrc.nist.gov/projects/cryptographic-module-validation-program/validated-modules/search)
and the [modules in process list](https://csrc.nist.gov/Projects/cryptographic-module-validation-program/modules-in-process/Modules-In-Process-List).
The AWS-LC module's security policy also lists the operating environments it
was tested on. For a build on another platform, the vendor-affirmation rules
apply.

`Cargo.lock` pins the module version. A dependency update that changes
`aws-lc-fips-sys` changes the module, so treat it as a crypto change.

## What the process checks

- **At start**, the process turns AWS-LC on in FIPS mode, which runs its
  power-on self-tests. It then installs rustls' FIPS configuration as the
  only TLS provider. If either step fails, the process stops (`fips::init`
  in `crates/ot-server/src/fips.rs`).
- **SAML** asks OpenSSL whether its default properties are `fips=yes`. If they
  are not, SAML sign-in is refused and the sign-in page doesn't offer it. A
  build run outside the image has no FIPS provider, so it has no SAML either.
- **TLS** uses rustls' FIPS configuration, so it offers only suites, key
  exchange groups and signature schemes the module approves. That means TLS
  1.2 and 1.3 with AES-GCM and NIST curves (plus any hybrid ML-KEM group the
  module approves).
  ChaCha20-Poly1305 and X25519 on its own are not offered.

## Algorithms

| Purpose | Algorithm |
|---|---|
| Passwords | PBKDF2-HMAC-SHA256 (SP 800-132), 600,000 iterations, 128-bit salt, 256-bit output, stored as `$pbkdf2-sha256$i=…$salt$hash` |
| Session and API tokens | HMAC-SHA256 (JWT HS256) keyed with `OT_SESSION_SECRET` or `session.key` (256 random bits) |
| SAML relay state | HMAC-SHA256 |
| External plugin handshake | HMAC-SHA256 challenge and response each way over two 256-bit DRBG nonces, keyed with the shared secret; verified in constant time |
| Random values | AWS-LC CTR-DRBG: token ids, the session key, generated passwords, SAML request ids, the page's style nonce (128 bits), plugin secrets and handshake nonces (256 bits) |
| NATS `.creds` | Ed25519 (FIPS 186-5) over the server nonce |
| Sync messages between nodes | Ed25519 (FIPS 186-5): each node's key pair is made by the module (`Ed25519KeyPair::generate_pkcs8v1`) and every message is signed and verified with it; key fingerprints and ids are SHA-256. AWS-LC's service indicator reports Ed25519 key generation, signing and verification as approved in FIPS mode (aws-lc-rs's `signature/tests/fips.rs`), so ECDSA P-256 was not needed |
| Plugin identity | SHA-256 |
| Client certificate status (OCSP) | Response signatures verified by `rustls-webpki` with the AWS-LC provider's algorithms; the request nonce from the DRBG; the RFC 6960 CertID's issuer name and key hashes are SHA-1 (`SHA1_FOR_LEGACY_USE_ONLY`), as the protocol defines them: an identifier, not a signature or security strength |

### SAML signatures

The OpenSSL FIPS provider verifies RSA and ECDSA signatures with SHA-2
digests. Set the identity provider to sign with RSA-SHA256 or stronger. Under
SP 800-131A, SHA-1 signatures are acceptable only for verifying legacy data,
so treat a provider still signing with SHA-1 as a finding.

### Passwords from earlier versions

Before 0.4.0, passwords were hashed with Argon2id, which is not an approved
algorithm. An old hash still verifies at sign-in. On that successful sign-in
it is replaced with a PBKDF2 hash, and the log records
`password hash upgraded to PBKDF2`. Accounts that never sign in keep their
Argon2id hash. An administrator can reset them or disable them; see the
administrator guide. The Argon2 code only ever *checks* those old hashes.

## Code outside the modules

A few crates in the dependency tree contain cryptography, but OpenTrack never
uses them for security:

| Crate | Why it is there | Status |
|---|---|---|
| `sha1` (via `tungstenite`) | The WebSocket handshake's `Sec-WebSocket-Accept` value, a protocol checksum (RFC 6455) | Not a security function |
| `sha2` (via `cranelift`) | Wasmtime's compilation cache keys | Not a security function |
| `ed25519-dalek`, `rand` 0.8 (via `nkeys`, inside `async-nats`) | async-nats' own `.creds` sign-in | Never called: OpenTrack reads the `.creds` file itself and signs with AWS-LC (`crates/ot-nats/src/creds.rs`) |
| `rand` 0.9 (via `samael`) | samael's default SAML request id | Replaced: OpenTrack overwrites the id with 128 bits from AWS-LC |
| `argon2`, `blake2` | Checking pre-0.4.0 password hashes | Verification only, until each account signs in once |

`ring` is not in the build. rumqttc 0.25.1 pulled it in through webpki
0.102, only for that crate's error type. `third_party/rumqttc` is that
release moved to webpki 0.103 without the `ring` feature. cargo-deny bans
`ring`, so CI fails if it comes back.

## Building

Every build compiles the FIPS module, so development hosts need **Go** and
**CMake** as well as clang. The Docker image installs pinned, checksummed
versions (see the `Dockerfile`). GitHub's Ubuntu runners already have them.
