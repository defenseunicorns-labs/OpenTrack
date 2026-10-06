# Threat model

OpenTrack 0.4.3, by interface and trust boundary, using STRIDE (spoofing, tampering,
repudiation, information disclosure, denial of service, elevation of privilege). For each
interface: the threats that matter, what mitigates them today (with the code or document that
shows it), what is left, and the POA&M item that covers it ([`ato/poam.md`](ato/poam.md)).
The last section is the criticality analysis.

**Review rule.** The maintainer re-reviews this document for every minor release (0.5.0, 0.6.0,
…), and in the pull request of any change that adds an interface, a trust boundary, a way of
signing in or a dependency on the security-critical list below. The review is recorded in that
release's CHANGELOG entry.

## System and data flows

One binary, `opentrack`, runs the roles `serve`, `sources`, `engine`, `writer`, `cot` and `link`
(`all` runs them in one process). Two stores hold its state: SQLite on the `/data` volume (everything
people configure or decide, accounts, the audit record) and Redis (everything the feeds produce).
Published tracks leave over NATS JetStream and TAK. Nodes share a picture through NATS subjects a
networking package carries.

```mermaid
flowchart LR
  subgraph users[Users]
    B[Browser]
    C[API client / script]
  end
  IdP[SAML identity provider]
  OS[OpenStare API]
  subgraph node[OpenTrack node: container, uid 1000, read-only root]
    CP[serve: UI and REST API<br/>OT_BIND 8090]
    SRC[sources: transports, codecs,<br/>pipelines, trackers]
    ENG[engine: correlation,<br/>track management]
    WR[writer]
    COT[cot: TAK output]
    LNK[link: multi-node sync]
    PL[WebAssembly plugins<br/>wasmtime sandbox]
    DB[(SQLite /data)]
  end
  EXT[External plugin process]
  R[(Redis)]
  N[(NATS JetStream)]
  F[Feeds: TCP, UDP, gRPC, HTTP,<br/>WebSocket, MQTT, files]
  TAK[TAK Server / ATAK / WinTAK]
  OT[OpenTelemetry collector → SIEM]
  PKG[Networking package or<br/>opentrack bridge]
  TILES[Tile server]

  B -- HTTPS, session cookie --> CP
  C -- HTTPS, API token or client cert --> CP
  B -. redirect / POST .-> IdP
  CP -- HTTPS or HTTP /api/auth/me --> OS
  CP -- HTTPS --> TILES
  F -- inbound or outbound, TLS optional --> SRC
  SRC --- PL
  SRC -- JSON lines, no TLS --> EXT
  SRC -- observations --> R
  CP -- commands --> R
  ENG <--> R
  CP --- DB
  ENG --- DB
  WR -- outbox --> R
  WR -- tracks.> --> N
  COT -- outbox --> R
  COT -- CoT XML: TLS, TCP, multicast --> TAK
  LNK <-- ot.sync.* --> N
  N <--> PKG
  node -- OTLP, TLS --> OT
```

**Trust boundaries**, from least to most trusted:
1. The network in front of the control plane, and the feeds and TAK clients.
2. Signed-in users, by role: `viewer < track_manager < admin`.
3. The deployment's services: Redis, NATS, the collector, the identity provider, OpenStare, the
   networking package. OpenTrack authenticates to them and encrypts the link, but trusts what
   they say once connected.
4. Code OpenTrack runs: WebAssembly plugins (sandboxed), external plugins (not sandboxed, trusted
   as far as their host is).
5. The host: the container runtime, the `/data` volume, the environment (`OT_*` secrets).

## UI and REST API (control plane)

`crates/ot-server/src/control.rs`, `https.rs`, `auth/`. Sessions, API tokens, roles.

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| S | Password guessing, credential stuffing | Lockout after 3 failures in 15 minutes until an admin unlocks (`auth/stig.rs`); per-address rate limit, a burst of 5 then one a second (`auth/mod.rs`); PBKDF2-HMAC-SHA256, 600,000 iterations; 15-character, four-class policy | No check against a compromised-password list | P-15 |
| S | Stolen session or token replayed | Server-side session record, idle timeout 15 minutes (admins 10), 24 h maximum, 3 per account, revocable (`auth/sessions.rs`); cookie `HttpOnly`, `SameSite=Strict`, `Secure` over TLS, ends with the browser; HSTS; API tokens signed with the session key (HS256), each with a stored record that expires and can be revoked, voided when the account is turned off or its password set | A token stolen from a script's host works until revoked or expired | |
| T | Cross-site request forgery | `SameSite=Strict` session cookie; a change made with a cookie must carry the `X-OpenTrack-CSRF` header the page sends (another site's form can't set a header, and CORS is never allowed); any change whose `Origin` (or `Referer`) is another site is refused, sign-in included; SAML's consumer is the one cross-site post (`auth/csrf.rs`); API tokens are headers; `frame-ancestors 'none'` and `X-Frame-Options: DENY` | Behind a proxy that rewrites `Host` without `X-Forwarded-Host`, `OT_PUBLIC_URL` must name the public address or the page's changes are refused | P-31 |
| T | Script injection into the UI | CSP: script from this origin only, no inline or evaluated script; styles from this origin or a `<style>` with the page's nonce, fresh on each load, so injected markup can't style the page either (`control.rs`, `CSP`, `page_csp`); React escaping; no `dangerouslySetInnerHTML` ([coding-standards.md](coding-standards.md)) | | P-25 |
| R | A user denies a change | Every change is a decision with its actor; the audit record is append-only and hash-chained (`ot-store/src/audit.rs`), exported as `audit.record` to the SIEM | Refused requests, reads of security objects, sign-ins by certificate, token and OpenStare, and the client address on decisions are not audited | P-01, P-02, P-03 |
| I | Data read beyond a user's role | Every `/api/v1` route has a role (`auth/policy.rs`, tested); unlisted changes need an admin; source secrets masked for non-admins (`ot_core::secrets`); server errors answer with a reference only (`ApiError`); status detail for admins only | Viewers see the whole picture: OpenTrack does not filter by clearance (AC-16, by design); outputs carry no classification markings | P-10 |
| D | Request floods, large bodies | Body limits (axum default; explicit ones for imports, plugins, sheets); a rate limit per client on the whole API, 20 requests a second with bursts of 100, per account, API token or address, answered 429 with `Retry-After` and audited once a minute (`auth/rate.rs`); sign-in's own stricter limit | Volumetric floods and many addresses at once are the platform's (a proxy, the network) | P-31 |
| E | A user raises their own role | Roles only changed by admins; SAML gives at most `track_manager` unless **Allow admin**; SAML never takes a local account; `OT_AUTH=off` makes everyone admin but is excluded by the hardening checklist (F-4) | Administration shares the user listener unless `OT_ADMIN_BIND` puts it on its own (the main one then answers admin routes 404) | P-18 |

## SAML assertion consumer

`auth/saml.rs`, `third_party/samael`, libxmlsec1 and libxml2 through OpenSSL's FIPS provider.

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| S | Forged or altered assertion | Signature checked against the pasted IdP metadata only (F-3); the OpenSSL FIPS provider must be active or SAML is refused (`openssl_fips()`) | An IdP signing with SHA-1 would verify; the hardening checklist requires RSA-SHA256 or stronger | |
| S | Replay, or a response to a request OpenTrack never made | Relay state HMAC-signed by OpenTrack, request id matched, each assertion used once, 5-minute window | | |
| T | XML attacks (XXE, entity expansion, signature wrapping) | Responses over 256 KB or with a DTD refused before parsing; libxml2 strict, no network, no recovery (#58); samael's verification of the signed element | libxml2 and xmlsec are C libraries from the Debian base, with open scan findings | P-22 |
| E | IdP grants admin, or takes over a local account | **Allow admin** off by default; a matching local account is refused (F-1); the IdP is authoritative for a `saml` account's role | An admin who turns **Allow admin** on trusts the IdP's role mapping | |
| R | Refused sign-ons unrecorded | Every refusal is audited with its reason | | |

## Client certificates and OpenStare sign-in

`auth/mod.rs`, `auth/openstare.rs`, `ot-source/src/tls.rs`.

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| S | A revoked or foreign certificate signs in | Path validation to `OT_TLS_CLIENT_CA`; OCSP (AIA or `OT_TLS_CLIENT_OCSP_URL`), answers signed by the CA or its delegated responder, then CRLs (`OT_TLS_CLIENT_CRL`, reloaded on change); refused with neither, on a stale list or unknown status; only mapped CNs sign in, to active accounts | OCSP usually travels over plain HTTP, so it relies on the response signature; the nonce is optional (a responder may omit it), so a replayed answer is bounded only by its nextUpdate (or an hour); certificate sign-ins are not audited | P-26, P-02 |
| S | OpenStare's answer is spoofed | Off by default; OpenTrack asks OpenStare's `/api/auth/me` itself; roles only by the mapping (unlisted roles refused) | Default API URL is plain HTTP on loopback; over a network it must be HTTPS. A compromised OpenStare signs users in here up to the mapped role, for up to 30 s after a revocation | P-02 |

## Source transports

`crates/ot-source/src/transport.rs`, `tls.rs`, `grpc.rs`, `frame.rs`, codecs and pipelines.

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| S | An unauthorised device feeds false tracks to a listener | A listener must authenticate its senders: `tcp_server` by TLS with a client CA (and CRLs), `grpc_server` by that or a bearer token (and a method allow-list). Otherwise it is refused, on save and at start, unless the source carries `"unauthenticated": "accepted"`, which the decision log records | UDP and multicast cannot authenticate; each such source, and any other accepted listener, is an exception the authorising official accepts | |
| S | A client connects to an impostor feed | TLS with the system roots or a private CA, and an optional server name; client certificates | `insecure_skip_verify` exists (F-4, excluded by the hardening checklist) | |
| T | Malformed or hostile input | Typed decoding; frame limit (4 MiB default); gRPC `max_message_kib`; malformed gRPC messages refused with INVALID_ARGUMENT; no `unsafe` in OpenTrack's code; pipeline reject stage | A feed that authenticates can still lie: correlation weighs it, a track manager can drop it | |
| I | Feed credentials disclosed | Secrets masked for non-admins; `${env:NAME}` keeps them out of the database; source changes audited masked (#62) | Inline secrets are in the database and its backups | |
| I | File transport reads arbitrary files | Files only under the data directory, checked when saved, probed, imported and started; probes audited (#63) | | |
| D | Flooding a listener | `grpc_server` `max_connections`; per-source Redis stream bounded by `OT_OBS_WINDOW_SECS`; one source cannot stop another | UDP floods are the network's to stop | |
| E | An admin-configured URL used to reach internal services (SSRF) | Only admins create sources or probe (`/probe` is an admin route) | An admin can point a source anywhere the host can reach: that is the feature | |

## Plugins

`crates/ot-plugin/` (`wasm.rs`, `grants.rs`, `external.rs`), [docs/plugins.md](../plugins.md).

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| T | A malicious or buggy WebAssembly plugin reads files, calls out or exhausts the host | wasmtime sandbox; WASI grants: no directories, network or environment unless granted; memory ceiling (256 MB default); call timeout by epoch interruption (5 s default), the instance discarded after; one instance per stream | wasmtime is a large dependency; a sandbox escape there would be a host compromise | |
| T | A replaced plugin file | Only admins add or replace; each change is in the decision log with the component's SHA-256; loaded before it is stored | | |
| S, I, T | An external plugin impersonated or its traffic read | Only admins add one; call timeout; on every connection OpenTrack and the plugin prove they hold a shared secret (HMAC-SHA256 challenge and response each way, role-labelled so a reflected challenge fails); one that fails is refused, logged and audited; without a secret only a Unix socket under the data directory is reached | JSON lines with no TLS: after the handshake traffic can be read or changed on the way; the secret is stored in the database (OpenTrack must answer with it); the process is not sandboxed. Keep external plugins on the same host (loopback or a Unix socket) | P-28 |

## Redis

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| S, T | Someone with Redis access injects observations or engine commands (`tms:cmd`), bypassing the API's role checks | `rediss://` with a CA and mutual TLS; a password; keys under one namespace | Anyone who can write to OpenTrack's namespace can change the picture: Redis is inside OpenTrack's trust boundary. Give it its own ACL user and network | |
| I | The live picture read from Redis | TLS; Redis ACLs (the site's) | Redis data at rest is unencrypted unless the platform encrypts it | |
| D | Redis down or full | Roles reconnect; the observation window bounds each stream; `/api/v1/status` reports it | Redis sizing is the site's | |

## NATS output

`crates/ot-nats/`, `writer.rs`, [docs/nats-output.md](../nats-output.md).

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| S | Someone publishes false tracks on `tracks.>` | NATS authorisation (the site's); OpenTrack authenticates with `.creds` (signed with AWS-LC), a token, or mutual TLS | Consumers trust the subject; NATS permissions must allow only OpenTrack to publish there | |
| I | The picture read in transit | `tls://`, `OT_NATS_CA`, mutual TLS | | |
| I | Published tracks carry no classification marking | Labels travel in the message's security fields | Outputs are unmarked | P-10 |

## Multi-node sync

`link.rs`, `bridge.rs`, `crates/ot-sync/`, [docs/multi-node.md](../multi-node.md), [docs/sync-icd.md](../sync-icd.md).

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| S | A rogue node sends reports or decisions (merges, deletes), claiming a trusted site | Every message is signed with the sender's Ed25519 key; a message is accepted only from a site in **Trusted nodes** with a key an admin pinned, and only if the signature verifies. Refusals are counted and logged; a failing signature from a trusted site is audited (`sync_signature_refused`). Pinning a key is an admin's decision, audited | A captured node's key signs as that node until an admin removes its key everywhere. A trusted node can relay (so could invent) decisions under another trusted site's id: entries are not signed one by one, but any node may decide anyway | |
| T | Tampered, replayed or reordered messages | The signature covers envelope and body; a message whose clock is more than 5 minutes off is refused, and an accepted signature is refused again for 10 minutes; HLC ordering, global decision ids (duplicates harmless), anti-entropy repair | Nodes' clocks must agree to within 5 minutes | |
| D | A flood of forged messages | Refused before decoding the body; logs and audit rate-limited to once a minute per site | Verification costs CPU and refused messages still use the link; keep strangers off it (the package) | |
| I | The picture read on the link | The package's encryption; `opentrack bridge` connects to each node's NATS over TLS (FIPS provider) with a CA, a client certificate (mutual TLS) and `.creds`, user and password or token, per node if they differ; secrets from files or the environment, never logged | Beyond the NATS servers, the link is only as private as the package or the bridge's TLS; a bridge run without `tls://` or a CA sends in the clear | |
| E | Users of an unattended node (a drone) change the shared picture | **Receive only** accepts no track management from the node's own users | | |

## TAK output

`crates/ot-server/src/cot/`, admin guide [TAK output](../guides/admin.md#tak-output).

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| I | The picture read by anyone on the path, or by anyone who connects to a listening output | TLS to TAK Server with a client certificate; listening outputs with TLS, a client CA and CRLs; the panel marks each output TLS or plaintext and the role warns when a plaintext one starts | Plain TCP and multicast (always plaintext) are available for protected networks; the hardening checklist requires a documented risk acceptance | |
| I | Events carry no classification marking | | CoT events are unmarked | P-10 |
| S | A client impersonates the server | Clients verify OpenTrack's certificate | | |
| D | A slow client holds up the others | Per-client queue of 8,192 events; a client that falls behind is dropped and counted | | |

## OpenTelemetry export

`telemetry.rs`, admin guide [OpenTelemetry](../guides/admin.md#opentelemetry).

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| I | Logs and audit records read in transit | `https://` endpoints on the FIPS module, a private CA, mutual TLS, headers | An `http://` endpoint is plaintext; the hardening checklist requires `https://` | |
| I | Secrets in logs | Secrets are never logged ([coding-standards.md](coding-standards.md)); server-error detail goes to the log, not the caller | | |
| R, D | The record lost when the collector is down | The audit table keeps every record; the gap is refilled from `GET /api/v1/audit`; the outage is shown in status | Alerting on silence is the SIEM's | |

## SQLite and the data volume

`crates/ot-store/`, admin guide [Data directory](../guides/admin.md#data-directory), [Configuration export](../guides/admin.md#configuration-export).

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| I | Theft of the volume or a backup: password hashes, inline feed secrets, plugin files, `session.key` | `session.key` and `initial-admin.txt` mode 0600; only `/data` writable; tokens themselves never stored | Encryption at rest is the platform's (SC-28) | |
| T | Audit rows edited or deleted | Triggers refuse UPDATE and DELETE; SHA-256 hash chain; `GET /api/v1/audit/verify`; chain head logged hourly, so a cut tail shows in the SIEM | Someone with the file can rebuild the chain; the exported copy is the check | |
| I | Configuration export leaks secrets | Admins only; audited (`export_config`); `Cache-Control: no-store`; CLI file mode 0600 | The file must be kept like the database | |
| E | Forged sessions from a stolen `session.key` | Keep it (or `OT_SESSION_SECRET`) in the secret store; changing it voids every session and token | | |

## Container, image and supply chain

`Dockerfile`, `docker-compose.yml`, [supply-chain.md](supply-chain.md), [scm-plan.md](scm-plan.md).

| | Threat | Mitigations | Residual risk | POA&M |
|---|---|---|---|---|
| T | A tampered image | Release images signed with the project's cosign key and verified in the release process; SLSA provenance and SBOM attestations; base images by digest; Go and OpenSSL source checked by SHA-256 | Signing uses cosign's own (non-FIPS) cryptography and a file key | P-17 |
| T | A malicious or vulnerable dependency | Lockfiles and `--locked`; cargo-deny (RustSec, licences, crates.io only, banned crates); `npm audit`; forks reviewed with notes; CI actions pinned by commit | Advisories are checked on each push and release, not on a schedule; while GitHub Actions is unavailable the checks run on the test server by hand | P-23 |
| T | Unreviewed changes on `main` | Branch protection: pull requests, one approval, required checks, no force-push | Admins may bypass; with one maintainer, merges currently use the bypass ([scm-plan.md](scm-plan.md#repository-and-branches)) | |
| E | Container escape or privilege use | Non-root (uid 1000), read-only root, every capability dropped, `no-new-privileges` | The sample compose uses host networking, a bind mount and no resource limits | P-21 |
| — | Known vulnerabilities in the base image | Each release image scanned (Trivy, Grype) | Debian 12 packages with unfixed Critical and High findings | P-22 |

## Criticality analysis

The components whose failure or compromise would break a security property, and why. They get
the closest review: a change to any of them names the component in its pull request, comes with
tests of the property, and is called out in the CHANGELOG. A dependency update that touches one
(for example a new `aws-lc-fips-sys`, `samael`, `rustls` or `wasmtime`) is treated the same way.

| Component | Where | Why it is critical | Level |
|---|---|---|---|
| Authentication and sessions | `crates/ot-server/src/auth/` (`mod.rs`, `api.rs`, `sessions.rs`, `password.rs`, `stig.rs`, `consent.rs`, `maintenance.rs`) | Decides who is signed in; enforces the STIG account policy, lockout, session limits, inactivity, consent | High |
| Authorisation | `auth/policy.rs`, `auth::layer` | The only place a route's role is decided; a wrong line exposes or opens a route | High |
| FIPS cryptographic boundary | `crates/ot-server/src/fips.rs`; `aws-lc-rs` / `aws-lc-fips-sys`; rustls FIPS provider; the OpenSSL 3.0.9 FIPS provider and `docker/openssl-fips.cnf` in the image | All cryptography; the validated-module claim (SC-13, IA-7) rests on it ([fips.md](fips.md)) | High |
| SAML | `auth/saml.rs`; `third_party/samael`; libxmlsec1, libxml2 | Parses untrusted XML and decides sign-in from a signature | High |
| TLS configuration | `crates/ot-server/src/https.rs`; `crates/ot-source/src/tls.rs` (client and server TLS, CRLs); TLS in the NATS, Redis, MQTT, TAK and OTLP clients | Confidentiality and peer authentication of every link; revocation checking | High |
| Audit | `crates/ot-store/src/audit.rs` (append-only table, hash chain, verify); `telemetry.rs` (export) | Accountability; the evidence of what happened (AU family) | High |
| Plugin sandbox | `crates/ot-plugin/src/wasm.rs`, `grants.rs`; `wasmtime` | Runs third-party code in the server process | High |
| Secrets masking | `crates/ot-core/src/secrets.rs` (re-exported as `ot_source::secrets`); `crates/ot-store/src/sources.rs` | Keeps feed credentials from non-admins and from the decision log and audit record | High |
| Configuration export and import | `crates/ot-server/src/config_backup.rs`, `crates/ot-store/src/backup.rs` | Reads and writes every secret and account at once | High |
| Forked crates | `third_party/samael` (SAML, FIPS check, quick-xml fix), `third_party/rumqttc` (no `ring`) | Carry security fixes that upstream does not; must be kept current by hand (their `OPENTRACK.md`) | High |
| Input handling | `crates/ot-source/src/frame.rs`, `transport.rs`, `grpc.rs`, codecs; `ot-codec-stanag4607`, `ot-sapient` | Parse untrusted network data | Medium |
| Security headers and errors | `crates/ot-server/src/control.rs` (`CSP`, `security_headers`, `ApiError`) | Browser protections; no internal detail to callers | Medium |
| Multi-node trust | `crates/ot-server/src/link.rs`, `crates/ot-sync/` | Applies other nodes' decisions to this node's picture | Medium |
| Release signing | `.github/workflows/release.yml`, `cosign.pub`, the signing key | Lets a site prove the image is the project's | Medium |

The mission functions (correlation in the engine, the writer's publishing) matter for the
picture's integrity rather than for security; their correctness is covered by tests and the
benchmark, not by this list.
