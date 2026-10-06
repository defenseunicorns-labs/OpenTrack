# Deployment hardening

This is the checklist for deploying OpenTrack 0.4.7 to an accredited environment. The controls
themselves are described in the administrator guide, under
[Security hardening](../guides/admin.md#security-hardening). How they map to NIST 800-53 is in
[stig-mapping.md](stig-mapping.md).

## Before first start

1. **Use a signed image.** Verify a release image before running it:
   `cosign verify --key cosign.pub --insecure-ignore-tlog=true ghcr.io/kineticlogic-io/opentrack@<digest>`
   ([supply-chain.md](supply-chain.md)).
2. **Use the image.** Only the image has OpenSSL's FIPS provider, so only it has SAML under FIPS.
   Its runtime is distroless Debian 12 (`gcr.io/distroless/cc-debian12`, non-root): no shell, no
   package manager, and of the operating system only glibc, OpenSSL 3, CA certificates and the
   libraries the binary loads. It runs as uid 1000. Its base images are pinned by digest. Scan the
   built image with your scanner (for example `grype` or `trivy`) and keep the report
   ([supply-chain.md](supply-chain.md)).
3. **Run it hardened.** `docker-compose.yml` already sets:
   - `read_only: true`, with `tmpfs` for `/tmp` and `/home/opentrack`;
   - `cap_drop: [ALL]`;
   - `security_opt: no-new-privileges`;
   - a bridge network with only port 8090 published (`OT_BIND_PORT`); Redis on an internal network
     with no route out and no published port; NATS reached on the host through `host-gateway`;
   - a named volume for `/data`;
   - `mem_limit`, `cpus` and `pids_limit` on OpenTrack and Redis (size them:
     [admin guide, Docker compose](../guides/admin.md#docker-compose)).

   Keep them. The only writable volume is `/data`. Redis in the same file runs as its own user,
   read-only, with no capabilities and `no-new-privileges`.

   **Host networking is an exception.** A site that must run OpenTrack on the host network (the
   override in the [admin guide](../guides/admin.md#upgrading-a-host-network-deployment)) exposes
   every port OpenTrack opens and shares the host's network namespace. Record it as a risk
   acceptance, and restrict the host's ports with its firewall.
4. **Set the first admin** with `OT_ADMIN_EMAIL` and a policy-compliant `OT_ADMIN_PASSWORD`, or use
   the temporary password in `initial-admin.txt`, which the first sign-in forces you to change.
   An `OT_ADMIN_PASSWORD` is not forced to change: choose it as the account's real password, and
   unset the variable once the account exists.
   Then delete `initial-admin.txt`.
5. **Set `OT_SESSION_SECRET`** (32 characters or more) from your secret store, or protect
   `session.key` in the data directory. Anyone with it can mint sessions.
6. **Never set `OT_AUTH=off`** outside a laboratory. It makes every caller an admin; the server
   warns every minute and the UI shows a red banner.

## Separate administration

Set `OT_ADMIN_BIND` to an address on the management network (or `127.0.0.1:…` behind a jump
host), so the admin interface is on its own listener and the users' address has none (SC-7,
SC-2): the admin routes answer 404 there. Publish that port to the management network only,
and allow only administrators' hosts to reach it with the host or network firewall. Give it its
own certificate and client CA (`OT_ADMIN_TLS_*`) if administrators use different PKI.
[Admin listener](../guides/admin.md#admin-listener).

## Encrypt every link

| Link | Settings |
|---|---|
| Browsers and API | `OT_TLS_CERT` and `OT_TLS_KEY` (optionally `OT_TLS_CLIENT_CA` for CAC/PKI client certificates, checked by OCSP (each certificate's AIA responder, or `OT_TLS_CLIENT_OCSP_URL`; allow the outbound HTTP) with `OT_TLS_CLIENT_CRL` pointing at a directory of your CAs' CRLs, refreshed daily, as the fallback; with neither a certificate is refused), **or** a TLS proxy with `OT_PUBLIC_TLS=1` / an `https://` `OT_PUBLIC_URL`, so cookies are `Secure` and HSTS is sent |
| NATS | a `tls://` URL, `OT_NATS_CA`, and `OT_NATS_CERT` / `OT_NATS_KEY` for mutual TLS; prefer `.creds` or mTLS over a shared token |
| `opentrack bridge` | the same for each node's NATS: `tls://` URLs, `OT_BRIDGE_NATS_CA`, `OT_BRIDGE_NATS_CERT` / `OT_BRIDGE_NATS_KEY`, `OT_BRIDGE_NATS_CREDS`; per node after the URL (`;ca=`, `;cert=`, `;key=`, `;creds=`). Passwords and tokens from files (`OT_BRIDGE_NATS_PASSWORD_FILE`, `OT_BRIDGE_NATS_TOKEN_FILE`), not flags |
| Redis | a `rediss://` URL, `OT_REDIS_CA`, `OT_REDIS_CERT` / `OT_REDIS_KEY` (Redis `tls-auth-clients yes`) |
| Feeds | each source's transport TLS settings; don't use `insecure_skip_verify` |
| TAK output | Settings → TAK output: a TAK Server over TLS (8089) with a client certificate; a listening output with TLS and a **client CA** (and revocation lists) so only enrolled TAK clients connect |

**TAK outputs in plaintext expose the picture.** A plain TCP output (to a TAK Server's 8087 or a
listening output without TLS) sends every published track in the clear, and a plain listening
output lets anyone who can reach its port take the whole picture. **Multicast is always
plaintext**: anyone on the network segment (or as far as its TTL reaches) can read it. Use them
only on a network that is itself protected and accredited for the picture's classification, and
document the risk acceptance; keep multicast TTL at 1. The TAK output panel marks each output TLS or
plaintext, and the `cot` role logs a warning when a plaintext output starts. A labelled track's
events carry its marking (a `__security` element and the start of the remarks), but that marks
them; it does not protect them.

**Listening sources authenticate their senders.** A `tcp_server` source needs TLS with a
`client_ca_file` (mutual TLS, certificates required); a `grpc_server` source needs that or a bearer
`token`. A source that cannot (UDP, multicast included, has no TLS) or does not is refused unless it
carries `"unauthenticated": "accepted"`: anyone who can reach its port can then feed it tracks.
**Each such source is an exception needing the authorising official's acceptance**; record it
with the source's name, port and the network that protects it. The flag is saved with the source,
so setting or clearing it is in the decision log (before and after) and the source's history; the
source list marks it with an UNAUTHENTICATED badge. A listener saved before 0.4.5 without it is not
started until it authenticates its senders or the flag is set (its status says why).

TLS runs only FIPS-approved suites (docs/security/fips.md).

**Pin every peer's key (multi-node).** Sync messages between nodes are signed with each node's
Ed25519 key and accepted only from trusted nodes whose public key an admin has pinned in
**Settings → Nodes**. Compare fingerprints over a second channel when pinning, keep `sync.key`
readable by the service account only, and remove a lost or captured node's key on every other node
at once. Keep nodes' clocks within 5 minutes (NTP, GPS): older messages are refused as possible
replays. See the admin guide, [Signed sync messages](../guides/admin.md#signed-sync-messages).

**Open only the ports you use.** Every port, protocol and service OpenTrack listens on or connects
to, with its default and its TLS and authentication options, is in the admin guide's
[Ports, protocols and services](../guides/admin.md#ports-protocols-and-services) table. Register
the ones you enable in PPSM, and let only those through the firewall. Keep external plugins on
the same host (loopback or a Unix socket): their protocol has no TLS. Give each a secret
(Settings → General → Plugins → Secret; [plugins.md](../plugins.md#authentication)): OpenTrack
and the plugin then authenticate each other on every connection, and one without a secret loads
only from a Unix socket under the data directory.

## Sign-in

- **SAML:** the identity provider signs with RSA-SHA256 or stronger. Keep **Allow admin** off
  unless the identity provider should make admins.
- **Local accounts:** SAML never signs in to a local account (a matching email is refused), so
  keep local accounts to break-glass and service use.
- **Password sign-in:** with single sign-on in place, consider turning **Password sign-in** off
  (Settings → Security → Single sign-on). Keep one break-glass local admin, with **Never turn
  off** on its row in Settings → Users so inactivity doesn't turn it off.
- **Notice and consent:** turn on the DoD notice and consent banner (Settings → Banners) with
  your standard text, and the classification banner.

## Fixed values

The account policy is fixed at the STIG values; there is nothing to set:
- **Passwords:** 15 characters or more, all four character classes, 8 characters changed, 5
  remembered, a minimum age of 24 h and a maximum of 60 days.
- **Lockout:** 3 failures in 15 minutes lock the account until an admin unlocks it.
- **Sessions:** idle timeout 15 minutes (admins 10), 3 per account, 24 hours at most.
- **Inactivity:** accounts are turned off after 35 days, except those marked **Never turn off**.
- **Audit record:** kept forever.
- **API request rate:** 20 a second, bursts of 100, per account, API token or address (429
  with `Retry-After` beyond).
- **Cross-site checks:** a change made with the session cookie carries the page's
  `X-OpenTrack-CSRF` header; a change from another site's `Origin` is refused. Behind a proxy,
  pass `Host` through (or send `X-Forwarded-Host`), or set `OT_PUBLIC_URL`.
- **Content security policy:** no inline script; a `<style>` only with the page's nonce, fresh
  on each load; no `'unsafe-inline'`.

## Operate

- **Send the logs to your collector:** set `OTEL_EXPORTER_OTLP_ENDPOINT` (an `https://` URL,
  with `OTEL_EXPORTER_OTLP_CERTIFICATE`, and a client certificate if the collector asks for one)
  on every role ([admin guide, OpenTelemetry](../guides/admin.md#opentelemetry)). Route log
  records with event name `audit.record` to the SIEM. Alert on the SIEM side when OpenTrack stops
  sending, and watch **Overview → System status → Telemetry** (`telemetry` in
  `GET /api/v1/status`).
- **Review the audit record** at the interval your plan sets, in your SIEM. The API gives the
  record itself to admins (`GET /api/v1/audit`, CSV with `format=csv`; use it to fill the SIEM
  after a collector outage); check the chain with `GET /api/v1/audit/verify`.
- **Keep logs off the host.** The hourly chain head in the exported logs is what shows that the
  audit tail was cut.
- **Patch:** CI runs `cargo deny` (RustSec) and `npm audit`. Rebuild the image when either reports
  something, and when Debian 12 ships security updates for a package in the image (the distroless
  base, or a library it copies in; [supply-chain.md](supply-chain.md)).
