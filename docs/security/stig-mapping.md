# Control mapping (NIST 800-53 Moderate / ASD STIG)

Status of OpenTrack 0.4.7 against the controls an application-level assessment covers. Controls
are cited by NIST 800-53 number. The ASD STIG requirements that implement them are named in
words, because STIG V-IDs change between releases: check each against the STIG release your
assessor uses. "Inherited" means the hosting environment provides the control.

Status: **Met**; **Partial** (met with a stated limit); **Open** (see the findings below);
**Inherited**.

| Control | Requirement | Status | How / where |
|---|---|---|---|
| AC-2 | Account management: create, change, turn off, remove; record who did it | Met | Settings → Users, `opentrack user …`; every change is in the decision log and the audit record |
| AC-2(3) | Turn off inactive accounts | Met | 35 days (fixed); checked at sign-in and every 10 minutes; break-glass accounts exempt (Settings → Users → Never turn off) |
| AC-3 | Enforce approved authorisations | Met | Roles `viewer < track_manager < admin`, checked on the server for every `/api/v1` route (`auth/policy.rs`); a source's secrets are hidden from everyone but admins (`ot_source::secrets`) |
| AC-6 | Least privilege | Met | Three roles; SAML can't grant admin unless **Allow admin** is on, and never signs in to a local account; the container drops every capability |
| AC-7 | Limit failed sign-ins | Met | 3 in 15 minutes lock the account until an admin unlocks it (fixed; `opentrack user unlock` on the server when every admin is locked out); plus a per-address rate limit |
| AC-8 | System use notification | Met | Notice-and-consent banner (Settings → Banners), accepted at every sign-in and enforced by the server (the API refuses until it is accepted; `consent_accepted` audited); declining signs the user out |
| AC-9 | Previous sign-in notification | Met | After sign-in: the last sign-in and the failed attempts since |
| AC-10 | Concurrent session control | Met | 3 sessions per account (fixed); the oldest ends |
| AC-11, AC-12 | Session lock and termination | Met | Idle timeout 15 minutes (admins 10), counted from the user's own activity; absolute lifetime 24 hours; all fixed; sessions kept on the server and revocable; the session cookie ends with the browser |
| AC-16 | Security attributes | Partial | Labels per source are carried on every track. A fused track takes the highest classification, the union of the restrictions and the intersection of the releasability lists. Every output marks a labelled track with one portion marking (`SecurityLabel::marking`): the NATS label, the CoT `__security` element and remarks prefix, the UI track card and table, and every exported file (tracks, audit, registry, configuration), each also marked as a whole (admin guide, Markings). OpenTrack does not filter by clearance (a deliberate decision; enforcement is downstream) |
| AU-2, AU-3, AU-12 | Audit events and their content | Partial | Sign-in success and failure (reason, address), sign-out, lockout, unlock, session end and timeout, password change, account turned off, every decision; actor, time, outcome, detail. Outbound connections log their resolved destination (`peer_addr`) on connect and reconnect. Not yet: refused requests and changes, reads of security objects, certificate/token/OpenStare sign-ins, the address on decisions (POA&M P-01 to P-03) |
| AU-4 | Audit log storage capacity | Inherited | The collector and SIEM store the exported record ([admin guide, OpenTelemetry](../guides/admin.md#opentelemetry)); the local table is small (one row per event) and lives on the `/data` volume, sized by the host |
| AU-5 | Response to audit failure | Met | A sign-in that can't be recorded in the audit table is refused. A collector outage does not stop OpenTrack: each role logs `OpenTelemetry export failing`, the Overview's Telemetry status turns red and `/api/v1/status` reports it; the table still holds every record. Alerting on the SIEM side is inherited |
| AU-6, AU-7 | Review, reduction and reports | Met / Inherited | Every audit row is exported over OpenTelemetry (log event `audit.record`: seq, actor, op, outcome, address, detail, hash) and logged on standard output (JSON with `OT_LOG_FORMAT=json`); review, correlation and reports in the SIEM are inherited; `GET /api/v1/audit` (filters, CSV; admins) |
| AU-9 | Protection of audit information | Met | Append-only table (triggers refuse UPDATE and DELETE), SHA-256 hash chain, `GET /api/v1/audit/verify`, chain head logged hourly |
| AU-9(2) | Audit backup on a separate system | Met / Inherited | Each record is sent to the collector as it is written (OTLP over TLS, mutual TLS available); the separate system's storage and protection are inherited |
| AU-11 | Retention | Met / Inherited | The local table is kept forever (fixed; nothing deletes rows); retention in the SIEM is inherited |
| CM-6 | Configuration settings | Met (accepted risk) | The account policy (passwords, lockout, sessions, inactivity, audit retention) is fixed at the STIG values, not configurable. `OT_AUTH=off` and per-feed `insecure_skip_verify` remain by the product owner's decision (F-4): the first warns every minute with a red banner; neither may be used in an accredited deployment (hardening checklist) |
| CM-7 | Least functionality | Met | One binary, one role per command; plugins sandboxed (WebAssembly, WASI grants) |
| IA-2 | Identification and authentication | Met | Local accounts, SAML, PKI client certificates, OpenStare trust, API tokens |
| IA-3 | Device identification and authentication | Met | Other OpenTrack nodes: each sync message is signed with the node's Ed25519 key and accepted only if it verifies with the public key an admin pinned for its site code; pins audited ([admin guide](../guides/admin.md#signed-sync-messages)) |
| IA-2(12) | PIV credentials | Met | Client certificates mapped to accounts by CN; revocation checked by OCSP (the certificate's AIA responder or `OT_TLS_CLIENT_OCSP_URL`; signed answers only, cached to nextUpdate), falling back to CRLs (`OT_TLS_CLIENT_CRL`, reloaded on change); with neither, or on a stale list or unknown status, the certificate is refused (fail closed) |
| IA-3 | Device identification and authentication | Partial | Listening sources authenticate their senders: `tcp_server` by mutual TLS, `grpc_server` by mutual TLS or a bearer token; a listener that does not (UDP and multicast cannot) runs only with the per-source `unauthenticated: accepted` risk acceptance, recorded in the decision log and needing the authorising official's acceptance ([hardening.md](hardening.md#encrypt-every-link)). External plugins and OpenTrack authenticate each other on every connection with a shared secret (HMAC-SHA256 challenge and response each way); only a Unix socket under the data directory may go without ([plugins.md](../plugins.md#authentication)) |
| IA-5(1) | Password-based authentication | Partial | 15 characters, four classes, 8 changed, 5 remembered, 24 h minimum / 60 days maximum age (all fixed), temporary passwords changed at first use, PBKDF2-HMAC-SHA256 (600,000 iterations). No check against a compromised-password list (POA&M P-15) |
| IA-5(2) | PKI-based authentication | Met | Path validation to the configured CA, then OCSP status with CRL fallback, as IA-2(12) |
| IA-7 | Cryptographic module authentication | Met | FIPS 140-3 modules only ([fips.md](fips.md)) |
| IA-8 | Non-organisational users | Inherited | Through the identity provider |
| MP-3 | Media marking | Met | Every file OpenTrack exports is marked: track GeoJSON (`security.marking`, and each labelled feature's) and CSV (first line, and a `classification` column), the audit CSV and registry sheets (first line or row; XLSX also each printed page), and the configuration export (`marking`). The marking is the highest label in the file, with the classification banner's text for unlabelled content (admin guide, Markings) |
| SA-11, RA-5 | Developer testing, vulnerability scanning | Partial | CI: tests, clippy `-D warnings`, `cargo deny` (RustSec), `npm audit`; coverage measured for each release (`scripts/coverage.sh`); a threat model reviewed each minor release ([threat-model.md](threat-model.md)); each release image scanned with `scripts/ato/scan` (Trivy, Grype). The base image's OS packages carry unfixed findings (POA&M P-22); nothing scans on a schedule yet (P-23) |
| SC-5 | Denial-of-service protection | Met | A request rate limit per client on the whole API: 20 a second, bursts of 100, per account, API token or (before sign-in) address; 429 with `Retry-After`, audited once a minute (`rate_limited`) (`auth/rate.rs`); sign-in's own stricter limit; body size limits. Volumetric floods are the platform's |
| SC-2 | Separation of system and user functionality | Met | With `OT_ADMIN_BIND` the admin interface is only on its own listener; the users' address answers the admin routes 404 and the UI hides them (hardening checklist) |
| SC-7 | Boundary protection | Partial | `OT_ADMIN_BIND` puts administration on a separate address for the management network; the firewall and network boundary are inherited |
| SC-8, SC-8(1) | Transmission confidentiality and integrity | Met | TLS for the UI and API, NATS, Redis, feeds and OpenTelemetry export, with mutual TLS available on each; HSTS. Sync messages between nodes signed (Ed25519) end to end, whatever carries them |
| SC-13 | Cryptographic protection | Met | AWS-LC FIPS 3.0 and the OpenSSL 3.0.9 FIPS provider; `ring` banned in CI |
| SC-17 | PKI certificates | Inherited | Your CA issues the server and client certificates |
| SC-18 | Mobile code | Met | Content-Security-Policy: this origin only, no inline script; a `<style>` element only with the page's nonce, fresh on each load (DRBG), and no inline `style` attributes in markup (F-5) |
| SC-23 | Session authenticity | Met | Signed session tokens with a server-side session record; `HttpOnly`, `SameSite=Strict` and `Secure` cookies; no framing; a change made with the cookie must carry the page's `X-OpenTrack-CSRF` header, and any change whose `Origin` is another site is refused (`auth/csrf.rs`) |
| SC-28 | Protection of information at rest | Inherited | Encrypt the `/data` volume and backups (the platform's disk encryption) |
| SI-2 | Flaw remediation | Met | Dependency advisories fail CI; rebuild on Debian security updates |
| SI-10 | Input validation | Met | Typed decoding for every source, size limits on bodies and messages; `unsafe` forbidden in OpenTrack's code; SAML responses with a DTD are refused before parsing and parsed strictly (no recovery, no network); file sources read only under the data directory |
| SI-11 | Error handling | Met | Server errors answer with a reference, their detail in the log; `/api/v1/status` details (paths, URLs, errors) for admins only |
| SR-3, SR-4 | Supply chain controls, provenance | Met | Lockfiles, pinned toolchain and base images, actions pinned by commit, SBOMs (CycloneDX); release images signed (cosign key) with SLSA provenance and SBOM attestations |

## Findings

The accreditation package ([`ato/`](ato/README.md)) evaluates every ASD STIG V6R4 and Container
Platform SRG V2R4 rule; its open items, with those below, are on the POA&M ([`ato/poam.md`](ato/poam.md)).


| ID | Finding | Status |
|---|---|---|
| F-1 | A SAML sign-in whose email matched a **local** account signed in as that account, with its role (admin even with **Allow admin** off). | **Fixed:** SAML refuses any account it didn't make ("a local account has this email", audited) |
| F-2 | Viewers could read full source specifications, including inline credentials. | **Fixed:** for everyone but admins, passwords, tokens, header and metadata values, URL credentials and secret-named fields in transport messages read `••••••` in sources, revisions (the decision log carries no specs); `${env:…}` references stay |
| F-3 | The IdP entity ID, sign-in URL and certificate fields in Settings → Security looked editable, but sign-ons are checked against the pasted metadata alone. | **Fixed:** the three are read-only values read from the metadata on every save and read (what older settings stored is replaced); to change them, paste new metadata |
| F-4 | `OT_AUTH=off` and per-feed `insecure_skip_verify` exist. | **Accepted** by the product owner; excluded by the hardening checklist |
| F-5 | The CSP allowed inline styles (`style-src 'unsafe-inline'`), which React, MapLibre and CodeMirror were thought to need. | **Fixed:** `style-src 'self' 'nonce-…'` with a fresh nonce per page load. React's, MapLibre's and the charts' styles are set through the CSSOM, which the CSP doesn't govern; CodeMirror and framer-motion take the nonce; the UI SDK's Help prose style gets it as the page's scripts make it |
| F-6 | Client certificates were not checked for revocation. | **Fixed:** OCSP (AIA or `OT_TLS_CLIENT_OCSP_URL`) with CRL fallback (`OT_TLS_CLIENT_CRL`), end-entity status, fail closed, hot reload |
| F-7 | Release images were not signed. | **Fixed:** `.github/workflows/release.yml` signs with the project key (no public log, the repository being private) and attaches SLSA provenance and SBOMs |
| F-8 | Turning an account off and on again revived its API tokens (a token issued in the same second as the turn-off survived it). | **Fixed:** turning an account off, by an admin or for inactivity, revokes its tokens for good |
| F-9 | `rust-toolchain.toml` followed `stable`. | **Fixed:** pinned to 1.98.1, here and in CI |
