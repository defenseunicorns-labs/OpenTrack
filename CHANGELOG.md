# Changelog

## 1.0.0, 2026-10-06

The first open-source release, under the MIT License. 1.0 carries everything up to 0.4.7, plus the
changes below.

### Changed

- **OpenTrack is open source under the MIT License** (`LICENSE`). Third-party components keep
  their own licenses, listed in `THIRD_PARTY_NOTICES.md`. The image carries the license at
  `/opt/opentrack/LICENSE`.
- **A shorter README**, with the detail moved to `docs/reference.md`, and a project website in
  `docs/pages` published to GitHub Pages.
- **The UI depends on `@kineticlogic/staresdk` from npm** ([stareSDK](https://github.com/kineticlogic-io/stareSDK),
  MIT) instead of a vendored build. `ui/vendor/` and `scripts/vendor-staresdk.sh` are gone.
- **The SAPIENT demo's compose file moved** to `docs/examples/sapient/docker-compose.yml`, with its
  Mosquitto configuration beside it.
- **The repository moved to `kineticlogic-io/OpenTrack`** (a GitHub transfer: issues, pull
  requests, releases and collaborators kept; the old URL redirects). Release images from the next
  release on are `ghcr.io/kineticlogic-io/opentrack`. Images and releases from before the project
  was open source have been withdrawn.

## 0.4.7 (alpha), 2026-10-02

Accreditation POA&M P-32 is closed, so the ASD STIG has no open findings: outbound connections
log their destination. Non-point contacts close their known limits where an ELNOT links them, and
acoustic arrays fuse (#27, correlation `correlation-6`). A symbol designer in the entity editor,
with domain, affiliation, CoT type and SIDC kept in step; track labels on the map; Track
Management and Sources panels fill the page; and a track manager's designation always applies.

### Added

- **The entity editor keeps domain, affiliation, CoT type and SIDC in step.** Changing the
  affiliation rewrites both codes' identity; changing the domain to another than the symbol's sets
  that domain's generic symbol; typing (or designing) a SIDC sets the CoT type, domain and
  affiliation, and typing a CoT type sets the SIDC (2525D), domain and affiliation, through the
  2525C/2525D crosswalk. A 2525C SIDC is stored as its 2525D equivalent.
  `scripts/registry/align-symbols.mts` aligns an existing registry the same way through the API
  (a dry run unless `--apply`): it fills the missing code and makes both follow an explicit
  domain and affiliation, never filling a blank one (blank means the feed's).
- **A symbol designer** in the entity editor (Edit on a track's card, or the Registry), #80. The
  flag button beside SIDC opens it: affiliation, then symbol set, entity, type and subtype, from
  the MIL-STD-2525D catalog (the `mil-std-2525` package, as OpenStare), point symbols only, with a
  live preview. **Use symbol** writes both the 2525D SIDC and the matching CoT type, taken from the
  2525C equivalent in JMSML's 2525C/2525D crosswalk (Apache-2.0, vendored as
  `ui/src/lib/milsym/crosswalk.json` by `scripts/vendor-symbol-crosswalk.py`). Started from
  OpenStare's tactical symbol builder; OpenTrack's own copy, without line and area graphics.
- **Track labels on the map** (#79): each track's name, or its track number when it has none.
  Labels that would overlap are left out (more fit as you zoom in); the selected track's is always
  shown. **Track labels** in Map display turns them off. The label font (Noto Sans Medium, SIL OFL
  1.1) is served by OpenTrack itself as glyph ranges (`ui/public/map-fonts`, vendored by
  `scripts/vendor-map-glyphs.sh` from the protomaps/basemaps-assets commit OpenStare pins).

### Changed

- **Non-point contacts close their known limits where an emitter identity links them** (#27),
  correlation `correlation-6`. An ELINT area now pairs with the track its ELNOT was found on (a
  ship whose own ESM lines carry it, or the fix those lines made) when that track fits the area
  clearly best, 10 to 1 with the ELNOT counting 10:1, on two successive reports. Position alone
  still never pairs a kilometres-wide area at open-water density. A fix track joins the track every
  one of its lines now reports for, rather than lingering beside it until it goes stale. A fix
  never holds two tracks of one sensor, which keeps apart two ships in line from that sensor. In
  the `esm-crossfix` scenario, ships with two tracks went from 3 to 1, wrong bearings from 26 to 0
  and ghost fixes from 3 to 0; with ELINT also on five AIS ships, from 7 to 2. No area pairs with
  another object. An emitter whose lines carry no ELNOT, known otherwise only by an ELINT area,
  still keeps two tracks. See `docs/non-point-contacts.md`.
- **Acoustic arrays fuse.** A bearing-only report goes to the track its sensor track's own ranged
  reports are on: an array that measures range now and then has already said which object the
  report is. A new scenario, `acoustic-arrays`, runs the five arrays of the SAPIENT demo against
  five drones (two in formation 80 m apart, one also on radar): every ranged and bearing-only
  report lands on its own drone's track (before: 1,596 of 4,545 ranged reports and 598 of 1,548
  bearings on a track also holding another drone), one track per drone.
- **Track Management and Sources panels fill the page.** The track map and track card stretch side
  by side and share the window's height with the track table, which scrolls inside; the track
  management log follows a scroll below. On Sources, the sources table takes the height the
  topology leaves. A closed panel keeps its title's height, and a short window scrolls the page.
  A filled table renders rows for its whole height, however many tracks it lists.

### Security

- **Outbound connections log their destination** (ASD STIG V-222470, POA&M P-32, #64). Each
  connect and reconnect logs, at INFO, the component and the resolved remote address in the field
  `peer_addr`: the socket's peer address for source TCP, WebSocket and gRPC clients, the TAK Server
  and multicast outputs and external plugins; the response's remote address, when it changes, for
  HTTP polling, OCSP, the OpenStare sign-in check and the basemap proxy; and what the configured
  host resolves to for NATS, the bridge, MQTT, Redis and OTLP collectors, whose libraries keep the
  socket. Source logs carry the source id. URLs are redacted or reduced to host and port.

### Fixed

- **The SAPIENT acoustic example no longer maps the node's object id as an identifier.** Each array
  numbers the objects it hears itself, so as an identifier the ids vetoed pairing one drone's
  tracks from several arrays (up to five tracks per drone). It stays the source track key.
- **An entity's affiliation outranks a source's country lists.** The Affiliation stage runs after
  the entity stage; it now leaves an affiliation the entity set (the entity is the authority), and
  says so in the designer's trace. Before, a designated friend on a feed whose flag was in no list
  became the lists' "otherwise" (unknown).
- **A track manager's designation always applies to the track.** Saving the entity editor opened
  from a track's card pins the entity to that track (identifier `track`), so it applies whatever
  the feeds' reports grade as, and follows the track through merges; the track's own identifiers
  stay on the entity for its later tracks. The editor says so before saving.
- **ADS-B designations corroborate.** A report with no name is graded on its callsign (at the
  default broadcast name field), so an aircraft's entity applies when its callsign matches; before,
  every ADS-B match was stale and nothing applied.
- **TAK output types** use the more specific of a track's CoT type and the one its SIDC gives
  (the CoT type on a tie). A 2525D SIDC translates only to identity and symbol set, so an entity
  storing both no longer loses its CoT type's function, and a feed's generic CoT type no longer
  hides a detailed SIDC.
- **Track map buttons** sit on a stareSDK `ButtonPalette` (its glass surface), stacked as
  OpenStare's map controls are, instead of floating transparent over the map.

## 0.4.6 (alpha), 2026-10-01

Reading a TAK Server: CoT framing and probe hints, a guided pipeline filter (to drop OpenTrack's
own tracks read back), ranged bearings from acoustic arrays, and a clearer track map.

### Added

- **A guided filter builder** in the pipeline designer's Filter stage. Keep only if and Drop if
  are built from rows of field, test and value, matched all or any. Fields come from the source's
  stored samples, with an example of each, and any other path can be typed. The tests read as
  words (is, is one of, starts with, does not contain, is at least, is missing…). The filter's
  effect on the stored samples shows as you edit ("drops 55, e.g. tms-OTK000058833"). Edit as JSON
  switches to the raw condition; one the rows cannot show (nested groups, transformed values)
  opens there.
- **Ranged bearings** (#74, kitplummer). A bearing observation may carry a measured range and its
  uncertainty (`range_m`, `range_sigma_m`); a complete polar report becomes a positioned
  observation with its covariance. SAPIENT decodes RFC 3339 protobuf timestamps. A five-array
  SAPIENT acoustic demo (`scripts/demo-sapient-acoustic.sh`, `docs/examples/sapient-acoustic-mqtt.json`).
- **Track map display options** (#74, kitplummer): hide the basemap; dim it; track size;
  high-contrast tracks with a selection halo; show or hide the selected track's evidence lines,
  sensor locations and uncertainty; fade the other tracks while one is selected. Kept per browser.

### Changed

- **CoT sources are framed by `</event>`.** In the add-source wizard and pipeline designer,
  choosing Cursor-on-Target XML on a TCP transport (or a TCP transport for it) switches the
  default line framing to the end tag `</event>`. CoT events arrive back to back, often over
  several lines, so line framing split them.
- **A silent probe says why it may be.** A probe that connects but captures nothing now reports
  the bytes it read and a hint: no bytes (the feed may be idle, the port may need TLS, or it may
  only take data in, like a TAK Server input), or bytes that never made a frame (check the framing).
- **Registry and Correlation panels fill the page.** The registry table, Correlation's Decisions
  and Suggestions take the height the window has instead of stopping at a fixed height; their
  tables scroll inside. Suggestions stretch beside the settings, so no gap is left above
  Decisions. A closed panel keeps its title's height.
- **The track map** draws bearing lines to their measured range, marks the sensors, always shows
  the selected track's error ellipse (as uncertainty), and outlines tracks in the theme's colours.
- **stareSDK** is vendored from OpenStare master (9e148d26), which adds `Select`.

## 0.4.5 (alpha), 2026-09-30

The rest of the accreditation POA&M (#64): the ASD STIG is down to one open item (P-32,
outbound destination logging), the Container Platform SRG to none.

### Upgrade notes

- **Nodes sign their sync messages** (ICD version 2): 0.4.5 nodes don't talk to earlier ones.
  Upgrade a swarm together, then pin each peer's public key in Settings → Nodes on every node.
  Back up `sync.key` (beside the database) with the database.
- **Listening sources must authenticate their senders**, or carry `"unauthenticated": "accepted"`
  (every UDP source needs it). An enabled listener without either doesn't start.
- **External plugins need a secret** (a mutual HMAC handshake); update the plugin to the current
  SDK, generate a secret in Settings → Plugins, restart the plugin with it. Without a secret only a
  Unix socket under the data directory is used.
- **Client certificates are checked by OCSP**, falling back to `OT_TLS_CLIENT_CRL`; with
  neither, a certificate is refused. Sites whose certificates name no OCSP responder set
  `OT_TLS_CLIENT_OCSP_URL` or `OT_TLS_CLIENT_CRL`.
- **Changes made from a browser must come from OpenTrack's page** (CSRF). Behind a proxy that
  rewrites `Host`, send `X-Forwarded-Host` or set `OT_PUBLIC_URL`.
- The track CSV export starts with a marking line and a `classification` column.
- Release images are signed with a new key made in the FIPS module: verify 0.4.5 and later with
  `cosign.pub`, earlier releases with `cosign-2026-09.pub`.

### Security

- **Markings (P-10):** each labelled track has one portion marking, e.g. `(S//REL TO USA, GBR)`.
  TAK events carry a `<__security …/>` detail element and remarks that start with it; track, audit,
  registry and configuration exports are marked; the UI shows each track's marking. Nothing is
  withheld by clearance.
- **Listening sources (P-16):** `tcp_server` by mutual TLS, `grpc_server` by mutual TLS or a
  token, or an explicit, audited risk acceptance shown in red.
- **FIPS release signing (P-17):** the OpenSSL 3.0.9 FIPS provider signs the cosign payload
  (`scripts/release/sign-image`).
- **Admin listener (P-18):** `OT_ADMIN_BIND` serves every admin function on its own address only.
- **IPv6 multicast (P-19)** for UDP sources and the TAK multicast output.
- **No inline styles (P-25):** the CSP drops `'unsafe-inline'`; each page gets a style nonce.
- **OCSP (P-26, #21)** for control-plane client certificates, with the CRLs as fallback.
- **External plugins (P-28)** prove a shared secret each way on every connection.
- **Bridge TLS and credentials (P-29)** for `opentrack bridge`.
- **Signed sync messages (P-30):** Ed25519, keys pinned in Settings → Nodes, replays refused.
- **CSRF and rate limit (P-31):** a header and Origin check on changes; 20 requests a second
  (bursts of 100) per account, token or address, 429 beyond.

## 0.4.4 (alpha), 2026-09-30

Closing the accreditation POA&M's 0.4 items (#64): the ASD STIG is down to 7 open items (all
planned for 0.5.0), the Container Platform SRG to none, and the POA&M to 13 items, four of them
new from the threat model (P-28 to P-31: external plugin channel, bridge TLS, sync site-code
trust, CSRF token and API rate limit).

### Upgrade notes

- **`docker-compose.yml` changed** (bridge network, named volume, its own Redis). A node that
  used host networking follows "Upgrading a host-network deployment" in the admin guide, or keeps
  host networking with the override it gives.
- **The image has no shell**: run tools as `docker compose exec opentrack opentrack ...`.
- New passwords that are common, or built from a common one, are refused.

### Accounts and audit

Closing the accreditation POA&M's account and audit items (#64):
- **Temporary accounts:** **Temporary** in Settings → Users (or `opentrack user add --temporary`)
  makes an account that is turned off 72 hours after it is made (fixed), with its sessions and
  API tokens, audited as `account_disabled` reason `expired` (P-13).
- **Common passwords refused:** a new password that is one of the 100,000 most common, or whose
  letters spell one, is refused; a site can add its own list as `common-passwords.txt` in the
  data directory (P-15).
- **More in the audit record:** a credential that identifies no one (`access_refused`), a role
  refusal (`access_denied`), a refused change (`change_refused`), an admin reading accounts,
  tokens, sign-in settings, the configuration export or the audit record
  (`read_security_object`), and the first use in an hour of each API token, client certificate
  and OpenStare identity (`login`). Decisions' audit rows carry the client address (P-01 to P-03).
- **Access log:** every API request is logged with target `access` (account, method, path,
  status, address, user agent, referrer, forwarded-for, duration), on standard output and over
  OpenTelemetry.
- Scheduled scanning (P-23) is accepted as release-time only: every release image is scanned
  when it is built.

### Security documentation

The development-process documents the accreditation review found missing (POA&M P-20, P-24,
P-27):
- **`SECURITY.md`**: how to report a vulnerability, supported versions (the latest 0.4.x),
  response targets, and how fixes and advisories are published.
- **`docs/security/scm-plan.md`**: software configuration management plan: configuration items,
  branches and branch protection (and the admin bypass while there is one maintainer and no
  GitHub Actions minutes), versions, the release process, change control, deployed nodes, roles.
- **`docs/security/threat-model.md`**: STRIDE per interface and trust boundary, with mitigations,
  residual risk and POA&M items, a data-flow diagram, and a criticality analysis; reviewed each
  minor release.
- **`docs/security/coding-standards.md`**: the rules the tools enforce and those checked in review.
- **Ports, protocols and services**: one table in the admin guide for PPSM registration, linked
  from the hardening checklist.
- **Test coverage**: `scripts/coverage.sh` (cargo-llvm-cov over the workspace); the summary is
  attached to each release. The Rust workspace at 0.4.3: 83.2% of lines, 80.6% of functions.
- Accreditation package: ASD STIG V-222632, V-222649, V-222653, V-222655, V-222657 and V-222670
  are not a finding (171 not a finding, 21 open); SSP RA-5(11), SA-4(9), SA-15(3) and CM-9
  implemented; the POA&M is down to 15 items.

### Deployment

- **The image runs on distroless Debian 12** (`gcr.io/distroless/cc-debian12`, non-root, pinned
  by digest): no shell and no package manager, and of Debian only glibc, OpenSSL 3, CA
  certificates and the libraries the binary loads, each still listed by package for scanners.
  FIPS is unchanged. Command-line tools run as the binary: `docker compose exec opentrack
  opentrack ...` (POA&M P-22).
- **`docker-compose.yml` is hardened** (POA&M P-21): a bridge network with only 8090 published
  (`OT_BIND_PORT`), a named volume for `/data`, memory, CPU and process limits (`OT_MEM_LIMIT`,
  `OT_CPUS`, `OT_PIDS_LIMIT`), its own Redis on an internal network, and NATS on the host through
  `host.docker.internal` (`OT_NATS_URL` defaults to `nats://host.docker.internal:4222`).
- **Upgrading a host-network node:** move `./data` into the volume, drop `OT_BIND`,
  `OT_DATA_DIR` and `OT_REDIS_URL` from `.env`, and point a `127.0.0.1` NATS URL at
  `host.docker.internal`. The steps, and an override that keeps host networking, are in the
  administrator guide under "Upgrading a host-network deployment".

## 0.4.3 (alpha), 2026-09-30

A security release: the fixes from the accreditation review, OpenTelemetry export, and the
accreditation package.

### Upgrade notes

- **Lockout holds until an admin unlocks the account** (was 15 minutes). If every admin is locked
  out, run `opentrack user unlock <email>` on the server.
- **The notice-and-consent warning is enforced by the server**: while it is on, every sign-in
  must accept it before the API answers (API tokens are not asked). Scripts that sign in with a
  password should use an API token.
- **File sources read only under the data directory** (`/data` in the image). A file source that
  points elsewhere is refused when saved or probed, and doesn't start: move its files under the
  data directory and edit its path.
- **Signing out, or closing the browser, ends the session**: the session cookie no longer outlives
  the browser.

### Security fixes

- Viewers and track managers could list account, API token and sign-in decisions through
  `/api/v1/decisions`; those are an admin's now (#57).
- The SAML assertion consumer refuses responses with a DTD or over 256 KB before parsing, and
  libxml2 parses strictly: no recovery from malformed XML, no network (#58).
- The session cookie is a browser-session cookie: no `Max-Age` (#59).
- The notice-and-consent banner: acceptance is kept and enforced on the server and audited
  (`consent_accepted`); the admins' **Go to Settings** way round it is gone (#60).
- Server errors answer with a reference and log their detail, instead of returning database and
  file-system text; `/api/v1/status` gives non-admins up or down only (#61).
- Source changes are recorded in the decision log and audit record with their credentials masked
  (the source's revisions keep them) (#62).
- File transports read only under the data directory, when saved, probed, imported or started;
  every probe is audited (`probe_source`) (#63).
- A locked account stays locked until an admin unlocks it (Container Platform SRG AC-7).
- Signing out says so on the sign-in page.
- The secret-masking rules moved to `ot-core` (`ot_source::secrets` re-exports them).

The accreditation package is re-evaluated: ASD STIG 165 not a finding, 27 open; Container
Platform SRG 100 and 21; POA&M 18 items (#64).

### Accreditation package

A generic DoD RMF package for sites taking OpenTrack to an ATO (#20), in `docs/security/ato/`:
- **SSP narratives** for all 287 NIST SP 800-53 Rev 5 Moderate controls (status and
  responsibility: OpenTrack, Shared or Site; `[[SITE: ...]]` placeholders).
- **STIG checklists** (`.ckl`) for the ASD STIG V6R4 (286 rules) and the Container Platform SRG
  V2R4 (188), every rule evaluated with evidence; site items left Not Reviewed with who answers.
- **Image scans** per release (`scripts/ato/scan`: Trivy and Grype), starting with v0.4.2.
- **POA&M** (27 items, Markdown and a CSV with the DoD template's columns).
- Generated from reviewed sources (`evaluations/`, `ssp/controls.json`, `poam.json`) by
  `scripts/ato/{checklist,ssp,poam}`.
- `stig-mapping.md` brought in line with the evaluation: AU-2/3/12, IA-5(1), SC-18, SA-11/RA-5
  are Partial. Admin guide and hardening guide corrections (API tokens after turning an account
  off, the first admin's password).

### OpenTelemetry

OpenTrack hands its logs, the audit record included, its traces and its metrics to the
deployment's OpenTelemetry collector (#50). It is a track management tool: storing, searching
and alerting on logs belong to the collector and the SIEM behind it.

- **OTLP export**, over gRPC or HTTP, configured with the standard `OTEL_*` variables (endpoint,
  protocol, CA and client certificate, headers, sampler, export interval, per-signal switches).
  Off until `OTEL_EXPORTER_OTLP_ENDPOINT` is set; TLS on the FIPS module.
- **Logs:** every log line, with its fields as attributes. Audit records are log events named
  `audit.record` (scope `audit`). The audit table, its hash chain, `/api/v1/audit` and
  `/audit/verify` stay as they are.
- **Traces:** API requests, source processing and writes, engine batches, writer publishes.
- **Metrics:** every Overview counter and gauge as `opentrack.<name>`.
- **Collector outages don't stop anything:** the role logs the failure once and the recovery,
  `/api/v1/status` gains `telemetry` (not counted in its 503) and **Overview → System status**
  a Telemetry row. A sign-in that can't be written to the audit table is still refused.
- The one-off commands (`migrate`, `user`, `config`, `plugin`, `retire`, …) log to standard
  error, so their output can be piped (#46).
- Docs: admin guide (OpenTelemetry), `stig-mapping.md` (AU-4, AU-5, AU-6, AU-9(2), AU-11),
  `hardening.md`.

### Fixes

- A static report that changes a vessel's details ships once `min_interval_secs` allows, not at
  the next heartbeat (#53).

## 0.4.2 (alpha), 2026-09-29

### TAK output

OpenTrack streams its published tracks to TAK as Cursor-on-Target, beside NATS (#35).

- **`opentrack cot`**, a new role (also run by `opentrack all`): reads the outbox in its own
  consumer group (`track-cot`), so a slow TAK link never holds up NATS, and sends exactly the
  tracks NATS gets. `OT_COT_CONSUMER`, `OT_COT_MIN_INTERVAL_SECS` (default 2 s per track;
  identity and classification changes at once).
- **Three deliveries, any number of outputs**, in **Settings → TAK output** (saved with the
  instance settings as `tak`, applied within 5 s without a restart): a **TAK Server**'s streaming
  input over TCP or TLS with a client certificate (reconnecting with backoff, the picture on every
  connect); **UDP multicast** SA (default `239.2.3.1:6969`, TTL, interface; one event per
  datagram); and **listening** for ATAK/WinTAK clients over TCP or TLS with optional client
  certificates and revocation lists (each client gets the picture, then the stream; a client more
  than 8,192 events behind is dropped). All TLS through the FIPS module.
- **Events:** `uid` `tms-<UID>`, `type` from the SIDC (2525C with function id, 2525D identity and
  symbol set, or a CoT type) or affiliation and domain, `how` `m-f`, `ce`/`le` from the position
  error (`9999999` unknown), course, speed, callsign, optional remarks (track number, sources);
  timed at the track's last report, stale 60 s (by default) after it, and re-sent every
  half of it until then, so a track that stops reporting leaves TAK instead of looking current. Ends are `t-x-d-d` deletes with a `link`
  to the uid and `__forcedelete`, stale at once. No label handling: tracks go out as they are.
- **Status and metrics:** `GET /api/v1/tak/status` and the panel (state, clients, events, errors,
  TLS or plaintext); `cot` in `/api/v1/metrics` (`sent`, `errors`, `dropped`, `clients`, per
  output too) and a TAK output chart on the Overview.
- `Sidc::cot_type` in `ot-core`: SIDC to CoT type.
- Docs: admin guide [TAK output](docs/guides/admin.md#tak-output), operator guide "In TAK",
  hardening (plaintext CoT and multicast expose the picture).

### Maps, sources and the pipeline designer

- **Basemap tiles:** Settings → General → **Basemap tiles** takes an XYZ raster tile URL (such as
  `https://tiles.example/{z}/{x}/{y}.png`); the Track Management and source preview maps then show
  those tiles instead of the country outlines. OpenTrack fetches them for the browser
  (`GET /api/v1/basemap/{z}/{x}/{y}`, any signed-in account), so the content security policy stays
  this origin only, internal tile servers work and a key in the URL never reaches browsers. Tiles
  are not cached on the server; browsers keep them for a day. `GET /api/v1/settings` gains
  `basemap_tiles` (on or off) and `settings.basemap_tiles_url`, empty for anyone but an admin; the
  decision log records only that a URL was set. stareSDK 0.1.9.
- **Pipeline designer, live preview by stage:** the Live preview pane now fills the designer's
  height and scrolls on its own. It shows the first 5 stored samples (decoded records, in order)
  as pretty-printed JSON as they are after the stage selected on the left: the frames they came
  from as received (Transport, each once), the record, what Reject kept, the observation from Map
  through Throttle, and at Publish the message as it would be published. A sample dropped on the
  way says where and why, and a tracker holding one for its scan says so. One line of counts and
  any errors sit above; the observations table is gone from this pane.
  `POST /api/v1/sources/validate` takes `trace: <n>` (at most 10) and answers with `trace`, the
  first n decoded records stage by stage; without it the response is unchanged, and ingest does no
  tracing work.
- **SAPIENT MQTT demo** (#44): `docker-compose.sapient.yml` (Mosquitto, Redis, NATS),
  `docs/examples/sapient-mqtt.json` (SAPIENT protobuf over MQTT, decoded by the `sapient` codec,
  tracked with GNN) and `scripts/demo-sapient.sh`, which starts OpenTrack with sign-in on, acts
  through a one-day API token for the first admin, and runs the SAPIENT simulator.

### Fixes

- **Fixed:** info tips inside a modal (the pipeline designer, Add account, Add plugin…) opened behind it (#45).

## 0.4.1 (alpha), 2026-09-29

### Settings, simplified

- **Settings in a new order:** General, Users, API tokens, Nodes, Data, Banners, Security labels,
  Single sign-on. **General** (was Instance) holds **Plugins**; **Data** holds the exports, the
  configuration import and **Purge**.
- **Purge** is one button that opens a confirmation, with **Also delete history** in it; no site
  code to type.
- **The account policy is fixed at the STIG values**, no longer a setting: passwords of 15
  characters or more with upper, lower, digit and special, 5 remembered, 8 characters changed, a
  24 h minimum and 60 day maximum age; 3 failed sign-ins within 15 minutes lock an account for 15
  minutes; sessions end after 15 minutes idle (10 for admins) and 24 hours at most, 3 per account;
  accounts not signed in for 35 days are turned off. The audit record is kept forever: the
  retention setting and its purge are gone. `/api/v1/auth/settings` no longer has
  `session_hours`, `password`, `lockout`, `sessions`, `audit` or `inactivity.disable_after_days`;
  saved settings and configuration files that still have them load, and those are ignored and
  dropped. Settings → Security loses the Sign-in, Password policy and Lockout and inactivity panels.
- **Settings → Security → Single sign-on:** SAML, OpenStare sign-in, client certificates and the
  Password sign-in switch are one panel with one Save.
- **Settings → Users → Never turn off:** break-glass accounts inactivity never turns off are
  marked on their row (still stored as `inactivity.exempt`). The all-sessions panel is gone; the
  account menu's own sessions and **Sign out everywhere** stay.
- **The audit record is reviewed in the server logs**: every row is also logged as an `audit
  record` event (target `audit`, with seq, actor, op, outcome, ip, detail, decision id and hash;
  a decision's before and after stay out of the log). Settings → Audit is gone;
  `GET /api/v1/audit` and `/api/v1/audit/verify` stay, for admins.
- **Security labels:** the classification order is a list you drag into order, add to and remove
  from (Settings → Security). The default is `UNCLASSIFIED`, `CUI`, `CONFIDENTIAL`, `SECRET`; a
  classification not in the list, such as `TOP SECRET` unless you add it, still ranks above them all.
  A node that already saved an order keeps it.

### Interface

- **Track Management** is the second tab, right after Overview.
- **Help** reads like a CODEX article: a rail with the guide choice (Operator / Administrator, with
  their section counts) and the contents (sections open to their sub-sections; the one you're in
  stays open), beside the guide itself, no longer inside a collapsing panel.
- **Bearing lines** on the map run from the sensor out to its maximum range (250 km when unset)
  along the great circle, with a faint ±σ wedge, instead of ending at the track (#17).
- Pipeline designer: the Decode stage shows a protobuf or plugin codec read-only (it is chosen on the
  Transport tab) instead of letting a format replace it (#3).
- Transport form: HTTP poll method and body, the byte order of length framing, the length field's
  adjustment and the gRPC server's keepalive (#6).

### Track management and correlation

- **Accepting a pair suggestion** merges with hold, as a merge from the track table does:
  correlation never splits it (split or undo still can) (#12).
- **Do not pair** in the track table, for two ticked tracks: correlation never pairs them; undoable
  from the management log, which now names the tracks (#4).
- **Correlation settings:** the split window and the local density radius can be set in the UI;
  the server refuses a split window that isn't positive or a negative radius (#5).
- **`OT_CORRELATION` overridden by saved correlation settings** now says so: the engine logs a
  warning at start when it is set and the saved approach differs (#14).

### Sources

- **SAPIENT codec** (BSI Flex 335 v2.0): a built-in `sapient` codec plugin (`ot-sapient`,
  decoding with the schema `sapient-rs` ships), framed on TCP by a 4-byte little-endian length
  prefix; example in `docs/examples/sapient.json`.
- SAPIENT codec: enum values inside map fields come out as names; a NaN or infinite float is `null`
  instead of failing the whole message (#30).

### Sign-in

- **An admin signed in by SAML can turn password sign-in off** (keeping SAML on in the same save);
  from a password session it is still refused (#10).
- **A `saml` account's role is the identity provider's:** Settings → Users shows it read-only, and
  `PUT /api/v1/auth/users/{id}` refuses to change it (409) (#11).
- **SAML's IdP entity ID, sign-in URL and signing certificate are read-only**, read from the pasted
  metadata at every save and read; to change them, paste new metadata (#8, finding F-3).

### Operations

- **Full configuration export and import** (#15). `GET /api/v1/export/config` (admins, audited,
  `Cache-Control: no-store`) and `opentrack config export` now write the whole configuration as
  `opentrack-config` version 2: sources with their secrets, schema versions, correlation, instance
  and sign-in settings, accounts with their password hashes and history, API token records, the
  registry, plugins (components included), imported tracker profiles and the track number counter;
  never the session key, sessions, the audit record or track state. `opentrack config import
  <file>` and `POST /api/v1/import/config` rebuild an **empty** node from it (refusing any other,
  with what it has), checking every section first and writing all of it in one transaction; the
  imported accounts replace the first admin. Settings → Data warns that the file holds
  secrets, and offers the import while the node is empty. See the admin guide, Backup and restore.
- **Track numbers are never issued twice after a restore** (#16): on start, before issuing any, the
  engine finds the highest track number of its site in use in SQLite, Redis and the NATS tracks
  stream, moves the counter past it (never back) and records a `uid_counter_advanced` decision.
  Tracks this node left in the stream that are no longer live (after Redis was lost) get a delete,
  recorded as a `stale_tracks_deleted` decision.
  The manual `UPDATE uid_sequences` step is gone from the restore procedure.
- **`/api/v1/status` answers 503** (same body) when SQLite, Redis or NATS is down, so a monitor
  that reads only the status code sees it; `/healthz` stays liveness only (#7).
- **Fixed:** the image's health check failed under docker compose without TLS (`opentrack health`
  refused the empty `OT_TLS_CERT` compose passes), so the container showed unhealthy.

## 0.4.0 (alpha), 2026-09-28

### Accreditation: DoD RMF / ASD STIG (800-53 Moderate)

Account, session, audit and web hardening, all configurable in Settings → Security with STIG
defaults.

- **Passwords** (local accounts): 15 characters with upper, lower, digit and special; the last 5 not
  reused; 8 characters changed; 24 h minimum and 60 days maximum age. An admin's password (new
  account, reset, `opentrack user passwd`) is temporary and must change at the next sign-in, as must
  an expired one: the session can do nothing else until then. Existing passwords keep working;
  their age counts from the upgrade.
- **Lockout:** 3 failures in 15 minutes lock an account for 15 minutes (or until an admin unlocks
  it: Settings → Users, `opentrack user unlock`, `POST /api/v1/auth/users/{id}/unlock`).
- **Server-side sessions:** idle timeout 15 minutes (admins 10), measured from the user's own
  activity, not the page's polling; at most 3 per account; list and end your own sessions (admins:
  anyone's) at `/api/v1/auth/sessions`. Sessions from before the upgrade end: sign in again.
- **Inactive accounts** (35 days) are turned off; break-glass accounts can be exempted.
- **Last sign-in notice:** the previous sign-in and the failed attempts since, after signing in.
- **Audit record:** a new append-only, SHA-256 hash-chained `audit` table with every decision
  (mirrored as recorded; undo still marks decisions in place) and every sign-in event, with reason
  and address. Sign-in fails closed when it cannot be recorded. `GET /api/v1/audit` (filters, CSV)
  and `GET /api/v1/audit/verify`; Settings → Audit. Optional retention with a verifiable anchor.
- **Web:** Content-Security-Policy, X-Content-Type-Options, X-Frame-Options, Referrer-Policy,
  Permissions-Policy, HSTS over TLS, `no-store` on the API. Session cookie `SameSite=Strict`;
  `Secure` also behind a TLS proxy (`OT_PUBLIC_TLS=1`).
- **Security labels:** a fused track (and a group) takes the highest classification of its sources
  (correlation setting `labels.classification_order`, default UNCLASSIFIED < CUI < CONFIDENTIAL <
  SECRET < TOP SECRET; U/C/S/TS accepted; an unknown one ranks highest), the union of their
  restrictions and the intersection of their releasability lists (`NONE` when empty). Before, it
  took the highest-priority source's label.
- **NATS TLS:** `OT_NATS_CA` (TLS required), `OT_NATS_CERT` and `OT_NATS_KEY` (mutual TLS).
- **`OT_AUTH=off`** now warns every minute and shows a red banner.
- Nodes sharing their profile must all run this version: older ones refuse correlation settings with
  `labels`.

### FIPS 140-3 cryptography, in every build

See [docs/security/fips.md](docs/security/fips.md).

- **AWS-LC FIPS 3.0** does all of the binary's cryptography: TLS everywhere (rustls' FIPS
  configuration), session and token signatures, password hashes, the NATS `.creds` signature and
  every random secret. The process won't start unless the module is in FIPS mode. Building needs
  Go and CMake.
- **Passwords** are hashed with PBKDF2-HMAC-SHA256 (600,000 iterations). Argon2id hashes from
  earlier versions still verify, and are replaced at the account's next sign-in.
- **SAML** signatures go through OpenSSL with only its validated 3.0.9 FIPS provider, which the
  image builds and forces. SAML sign-in is off wherever OpenSSL is not in FIPS mode.
- **No `ring`:** the rumqttc copy in `third_party/` drops it, and `cargo deny` bans it.

### Encryption in transit

- **Redis over TLS:** `rediss://` URLs, with `OT_REDIS_CA` and, for mutual TLS, `OT_REDIS_CERT`
  and `OT_REDIS_KEY`.

### Supply chain and image

See [docs/security/supply-chain.md](docs/security/supply-chain.md).

- **`cargo deny` in CI** (`deny.toml`): RustSec advisories, a licence allow-list, FIPS bans and
  crates.io only. It led to fixes for:
  - quick-xml in the SAML fork (RUSTSEC-2026-0194 and 0195);
  - webpki in the MQTT client (RUSTSEC-2026-0049, 0098, 0099 and 0104).
- **Other CI checks:**
  - `npm audit`;
  - CycloneDX SBOMs of the binary and the UI (`scripts/security/sbom.py`, uploaded by CI);
  - actions pinned by commit, with a read-only token.
- **The image:**
  - base images pinned by digest, and the Go toolchain checksummed;
  - `HEALTHCHECK`, run by the new `opentrack health`.
- **The compose file** runs the image with a read-only root file system, every capability dropped
  and `no-new-privileges`.
- **Deployment and assessment:**
  - a hardening checklist ([docs/security/hardening.md](docs/security/hardening.md));
  - a control mapping with the open findings
    ([docs/security/stig-mapping.md](docs/security/stig-mapping.md)).

### Findings closed

- **SAML never signs in to a local account:** a sign-on whose email matches an account SAML didn't
  make is refused (before, it signed in as that account, with its role, even admin).
- **Only admins see a source's secrets:** viewers and track managers get passwords, tokens, header
  and metadata values, credentials in URLs and API keys in transport messages as `••••••`;
  `${env:…}` references stay visible.
- **Client certificate revocation:** `OT_TLS_CLIENT_CRL` (files or directories of CRLs; also
  `tls.client_crl_files` on TLS server sources). Revoked, uncovered and stale-listed certificates
  are refused; the lists reload within a minute of a change.
- **Turning an account off revokes its API tokens** for good.
- **Signed releases:** publishing a release builds the image with SLSA provenance and an SBOM,
  pushes it to GHCR, signs it with the project's cosign key (no public log; verify with
  `cosign.pub`), and attaches the SBOMs and digest to the release.
- **Pinned Rust toolchain** (1.98.1).
- `scripts/github/protect-main.sh`: branch protection for `main` (pull requests with one approval,
  CI passing), to apply once the account can.

### Guides and in-app help

- **Guides:** an administrator guide and an operator guide, in `docs/guides/`.
- **Help tab:** the guides are also in the new Help tab, bundled into the build so they work
  offline, and each section can be linked (`#help/<guide>/<section>`).
- **Info tips:** about 160 new ⓘ tips across sources, the pipeline designer, tracks, the registry,
  the schema, correlation, metrics and settings. Tips that were only native tooltips became info
  tips, and stale text was corrected.

## 0.3.4 (alpha), 2026-09-28

### Non-point contacts

Lines of bearing, areas of uncertainty and emitter identities (ELNOT) are fused into tracks. See
[docs/non-point-contacts.md](docs/non-point-contacts.md).

- **The model:** an observation's `geometry` is a `bearing` (from the sensor's position, with its
  error and range) or an `area` polygon. Mapping destinations `geometry.bearing_deg`, `sigma_deg`,
  `max_range_m`, `elevation_deg` and `geometry.polygon`.
- **Association:** a bearing goes to a track when its emitter identity matches, or once its emitter
  has been found on the track: lines from three or more sensors pointing at its precise position,
  clearly likelier than at any other track, or a cross-fix that pairs with it. It adds evidence and identity
  but never moves the track. An area, or an ellipse over 2 km, pairs only with the one track it
  holds.
- **Cross-fixing:** bearings from several sensors are crossed by weighted least squares into fixes
  with an honest error ellipse. The fixes start or update tracks as reports of the built-in `fix`
  source. Ghosts are kept out by emitter identity, three-sensor consensus with outlier tests,
  repetition, and rejecting sets that other tracks already explain.
- **Output:** tracks carry `area` and `bearings`; bearings no track took are published live on
  `contacts.bearing.<source>.<key>`.
- **Map and track card:** a selected track's bearing lines and area; the card lists its bearings.
- **Benchmark** (`esm-crossfix`): 98.9% of 2,297 bearings to the right track, 10 of 10 emitters
  without AIS tracked, 0.44% ghost fixes, 92% of fixes within 2σ, 3 of 20 ships left with a
  duplicate track; ELINT areas never paired with the wrong track (38 paired, 62 alone). Videos render from the run (`scripts/benchmark/replay/esm-video.py`).
- **Single-sensor location:** one moving ESM sensor locates what it hears from its own motion. It
  fits a fixed emitter, or a moving one (bearings-only target motion analysis) when the bearings
  say it moves, with an error that counts what the model can't see. The location enters the
  picture as a `fix` report keyed by ELNOT, so ELINT and video converge on it. Scenario
  `esm-patrol`: a patrol aircraft's ESM locates a fast boat with no AIS at 285 s; its ELINT joins
  at 460 s and its video 15 s after it starts, leaving one track 20 m from the truth, with one
  track number throughout.
- **Emitter motion per source:** `emitter_motion` (`manoeuvre_mps2`, `max_speed_mps`) says how the
  emitters a bearing source hears may move; it bounds single-sensor location's error. Default:
  surface traffic, 0.1 m/s² and 30 m/s. Set in the source's Publish settings.
- **An ELNOT is evidence, not identity:** it never pairs or keys tracks by itself, adds 10:1 when
  two reports share one, and counts for nothing when they differ (a platform can carry several
  radars). Two lines of the same ELNOT fix only by repeating, like anonymous ones.
- **Bearings carry their platform's position:** every bearing must carry its sensor's position at
  its time, in the same message; OpenTrack does not join it from a navigation feed.
- **Correlation:** a track close to another no longer counts itself in the local density it is
  judged against, so a precise track (video) pairs with a wider one (ELINT) beside it.
- **Open water:** set `kinematic.object_density_per_km2` to the real density; the default (1 per km²)
  keeps kilometre-uncertain fixes from pairing by position.

## 0.3.3 (alpha), 2026-09-27

### Protobuf inputs over gRPC

A producer's `.proto` files are uploaded with the source and compiled when it runs, with no `protoc`,
no generated code and no rebuild. Its messages decode into records the mapping reads. See
[docs/protobuf-grpc.md](docs/protobuf-grpc.md).

- **The `protobuf` codec** works over any transport: TCP with varint length framing, UDP, MQTT,
  WebSocket, recorded files, or gRPC.
  - Field names stay as written in the `.proto`, 64-bit integers are numbers, and fields at their
    default are included (a latitude of 0 is 0).
  - A `Timestamp` is an RFC 3339 string.
- **`grpc_client`:** OpenTrack calls a producer's streaming method.
  - The request is written as JSON, and metadata can carry `${env:}` secrets.
  - TLS or mutual TLS, HTTP/2 keepalive pings, and a message size limit.
  - When the producer hangs up or the connection drops, OpenTrack calls again with jittered backoff.
- **`grpc_server`:** producers call methods of their own `.proto`.
  - A bearer token, TLS or mutual TLS (each record carries the producer's certificate subject), and
    limits on connections and message size, gzip bombs included.
  - Every message is checked against the schema. A bad one fails the call with INVALID_ARGUMENT
    saying why, rather than being dropped silently.
  - Backpressure through HTTP/2 flow control: nothing is dropped.
  - Refused calls and failed TLS handshakes show in the source's status.
- **Saving a source checks it:** the `.proto` compiles (errors give `file:line`), and the method fits
  the message.
- **Source editor:** add `.proto` files and see them compile; pick the message, the records field
  and the gRPC method. `POST /protobuf/describe` returns the same for the API.
- **An example:** `docs/examples/grpc/` has a producer on Google's `grpcio` and source specs for both
  directions.
- **Tested against `grpcio`:**
  - both directions, reconnecting after a producer restart, and mutual TLS;
  - 16,000 records a second pushed for a minute with no loss.
- **Fixed:** with NATS unreachable, shutting down waited forever on the multi-node link.

## 0.3.2 (alpha), 2026-09-27

Multi-node: several OpenTrack nodes (server sites, or a drone swarm) share one track
picture with no node in charge. The design is in [docs/multi-node.md](docs/multi-node.md), and the
messages the nodes exchange are specified in [docs/sync-icd.md](docs/sync-icd.md).

### One picture, one number, one reporter

- **Each node reports what its own sensors see**, never what it heard from other nodes, so no node
  gets its own data back disguised as a second sensor.
- **Reporting responsibility:** for each track, the node that sees it best (track quality 0–15,
  from its position error) reports it. The others hold it quietly.
  - A node claims a track only when it beats the reporter's quality by 2.
  - A reporter yields to a better report, and a silent one is taken over after 24 s.
  - A node leaving releases its tracks at once.
- **Dead reckoning sets the rate:** a track is sent when the others' prediction of it drifts past
  50 m (200 m in the air), and otherwise every 12 s.
- **Two numbers for one object become one.** Every node keeps the number with the older origin
  (then the lower UID), whatever it has published. A `delete` on NATS carries `merged_into`, so
  consumers can move their references.

### Track management holds on every node

- **What replicates:** pair, unpair, merge, do-not-pair, delete, groups, deleting a history point,
  and undo.
- **How:** each is logged under a global id (`<site>:<seq>`) and a hybrid logical clock stamp, and
  applied on every node.
  - Conflicting decisions settle the same way everywhere: the later decision on the same tracks
    wins.
  - A decision about a track a node has not heard of yet waits for it (up to 12 h).
  - Undo works across nodes.
- **Receive-only nodes** (a drone with no operator) apply other nodes' decisions and accept none of
  their own.
- **The profile is shared:** publishing an output schema or saving correlation settings applies on
  every node. A local schema draft survives another node's publish.

### Nodes exchange messages without trusting the network

- **OpenTrack does not do the networking.** It publishes compact binary messages on its own NATS
  (`ot.sync.out.*`) for a networking package to carry, and hears other nodes on `ot.sync.in.*`.
  - A track report is 40 bytes.
  - Messages are at most 1 KB.
  - Only site codes an admin trusts are listened to.
- **Missed decisions are repaired:** nodes compare which decisions they hold every 5 s and ask for
  gaps from whoever has them. A decision made on one drone reaches a node that was never in range
  of it.
- **`opentrack link`** runs the boundary. It is part of `all`, and idle until sharing is enabled.
- **`opentrack bridge`** carries the messages between server sites' NATS servers. It can also drop,
  delay, duplicate, cap and partition, to stand in for a poor link.
- **`docs/examples/two-sites`**: two sites and a bridge in docker compose.

### Measured: `bench swarm`

`bench swarm` runs K live nodes with overlapping sensors (AIS-like and radar-tracker-like) over the
bridge, and scores every node's published picture against the truth. The gate run used 5 nodes and
600 targets, with 10% loss, 300 ms jitter, 2% duplicate messages, and half the nodes cut off from
the other half for 5 minutes:

| | Result | Gate |
|---|---|---|
| Targets held under the same number on every node | 98.8% | ≥ 98% |
| Duplicate tracks | 0.76% | < 1% |
| One picture again after the cut heals | 2 s | ≤ 30 s |
| Track reports per node | 6.9 kbit/s per 500 tracks | ≤ 20 |
| Coverage (each node holds what any node sees) | 99.7% | |

On a thin link the same swarm was capped at 16 kbit/s per node, with 10% loss. Each node had a sending
budget of 14 kbit/s (Settings → Nodes). With the budget, 98.4% of what any node sees reaches every
node, and 96.6% of targets have one number everywhere; without it, 83% and 61%. The budget
sends the most urgent reports first: new tracks, state changes, then the largest drifts. A drift
within a track's own position error is not sent at all.

### Settings and UI

- **Settings → Nodes:**
  - share the picture, trusted nodes, receive-only, share the profile;
  - each peer's link health and the decisions held from it;
  - how many tracks this node reports.
- **The track card** shows which node reports the track.
- **The Management log** marks decisions made on other nodes.
- **`GET /sync/status`** returns the same, for headless nodes.
- **The Correlation tab** shows a bubble counting suggestions waiting for a track manager (the
  stareSDK Tabs `badge`).

### Upgrading

Migration `0014_sync` adds the replicated log. Nothing is shared until an admin turns sharing on,
and every node needs its own `OT_SITE_CODE`.

## 0.3.1 (alpha), 2026-09-27

Phase 2, "proven at scale", is met: 16,000 live tracks at 1 Hz on one host, and fewer than 2% wrong
pairings on the harbour benchmark.

### Throughput under live load

- **`bench live`** runs the whole server with a Redis and NATS of its own, fed N tracks at 1 Hz, and
  samples it every 10 s: backlogs, memory, CPU, publications a second, and the delay from report to
  publication.
- **16,000 tracks for 10 minutes:**

  | | Before | After |
  |---|---|---|
  | Engine backlog | 3.4 million, growing | at most 200 |
  | Published | 2,000/s | 3,242/s |
  | Delay (p50 / p99) | 142 / 218 s | 1.0 / 2.0 s |

  It uses about one core and 313 MB.
- **Fixed: the engine dropped reports under load.** A timer abandoned the batch in progress. Timers
  now wait until a batch is finished.
- **The writer publishes concurrently:** one Redis read for every track due, 256 publications in
  flight.
- **Deletes and history-point deletions have their own outbox stream.** It is never trimmed, so a
  writer that falls behind no longer loses them.
- **`OT_OBS_WINDOW_SECS`** sets how long observation streams keep reports (default 600 s). That is
  about 4.8 GB of Redis at 16,000 reports a second.

### correlation-5

- **Splits work against slow feeds.** A radar track that followed another vessel could stay on an
  AIS track that reports every 10 s indefinitely. The split check now has its own window,
  `split.window_secs`, and may reuse the other side's last report.
- **Local density:** a close approach counts for less in a crowded harbour
  (`local_density_radius_m`, 500 m).
- **Automatic splits are the default.**
- **The Autoferry lidar uses MHT**, and so does the `lidar-surface` profile.
- **Harbour wrong pairings: 5.36% → 1.51%.** The costs:
  - more radar and AIS tracks stay apart in the Solent (GOSPA 7,151 → 9,805)
  - more identity changes on two Autoferry recordings

  See [docs/algorithms.md](docs/algorithms.md).

### Benchmark

- **IDF1** for system tracks and each tracker.
- **The harbour gate** appears in every summary, with a stricter figure that judges each pairing at
  each moment.

### UI

- **The History tab** has "Show on map", which draws the track's history as a line, and "Zoom to
  track".
- **stareSDK 0.1.8:** MapView lines and camera fitting.

### Upgrading

- No migrations.
- **Correlation settings saved before 0.3.1 keep their `split.automatic`.** Turn it on in the
  correlation form.

## 0.3.0 (alpha), 2026-09-26

The "secure" phase: sign-in and roles, TLS, undo and history. NATS stays the only output.

### Sign-in and roles

- **Every API call needs a signed-in caller**, in one of three roles:
  - `viewer`: sees everything but accounts and secrets
  - `track_manager`: also manages tracks
  - `admin`: also configures OpenTrack
- **One middleware checks the role each path needs.** A new route that changes something is an
  admin's until it is listed. The decision log now records the signed-in account, which a client
  can no longer set.
- **How callers sign in**, modelled on OpenStare's:
  - local accounts (Argon2id passwords) with a signed `ot_session` cookie
  - SAML 2.0 single sign-on, ported from OpenStare
  - OpenStare's own sign-in and API tokens, when OpenTrack runs beside OpenStare
  - API tokens for machines
  - client certificates over TLS
- **UI:**
  - the sign-in page, like OpenStare's
  - the user menu (change password, sign out)
  - Settings → Users (accounts and API tokens)
  - Settings → Security (sessions, SAML, OpenStare sign-in, client certificates)
  - controls a role cannot use are disabled
- **Warning banner:** as OpenStare's, text users accept after signing in. Declining signs them out.
- **`opentrack user`** manages accounts on a node without the UI.
- **The first admin** comes from `OT_ADMIN_EMAIL` and `OT_ADMIN_PASSWORD`. Without them, it is
  `admin@opentrack.local`, with the password in `initial-admin.txt` beside the database.
- **`OT_AUTH=off`** turns sign-in off for development.

### TLS

- **The API and UI over TLS** with `OT_TLS_CERT` and `OT_TLS_KEY`. With `OT_TLS_CLIENT_CA`,
  client certificates sign machines in.
- **Feeds over TLS and mutual TLS:**
  - a `tls` object on the `tcp_client`, `http_poll`, `websocket` and `mqtt` transports: CA, client
    certificate and key, server name
  - a `tls` object on `tcp_server`: certificate, key, and a client CA that makes client
    certificates required

### Undo

- **What undo covers:** a track manager's pair, unpair, delete, merge, split, "do not pair" and
  group changes, from the decision log (`POST /decisions/{id}/undo`).
- **An undo is a decision of its own.**
- **How it works:** it reverses the decision's links in the temporal track graph.
  - A deleted or merged-away track comes back under its UID.
  - A track the engine made meanwhile for the same source retires.
- **When it is refused:** while a later change by someone else to the same tracks stands.

### Position history

- **What is kept:** each system track's published positions, for `history_hours` (default 12 h).
  At most one point is kept every `history_interval_secs` (default 10 s), both in Settings.
  - A point costs about 130 bytes: 2,000 tracks for 12 h at 10 s is about 1.1 GB of Redis.
- **Reading it:** `GET /tracks/{uid}/history`.
- **Delete history point** (`POST /history/{uid}/delete`): a track manager deletes a bad point.
  - It goes out on NATS as `delete_history_point`, on `tracks.history.tms-<UID>.<ms>`.
  - When the point was the track's latest, the track steps back to the point before.
  - **Consumers of `tracks.>` must skip message kinds they do not handle**
    ([docs/nats-output.md](docs/nats-output.md)).

### Engine

- **One SQLite connection** (0.2.2) and batched writes keep the engine at about 25,000 to 29,000
  observations a second from 2,000 to 16,000 tracks, with history recorded.

### Upgrading

- **Migration 0013** adds accounts, tokens and sign-in settings. It runs on start.
- **Sign-in is on after the upgrade.** Set `OT_ADMIN_EMAIL` and `OT_ADMIN_PASSWORD`, or read
  `initial-admin.txt` beside the database, then sign in.
  - Scripts calling the API need an API token.
- **The image** now needs libxmlsec1, which it installs. `--no-default-features` builds without
  SAML.

## 0.2.2 (alpha), 2026-09-26

### Engine throughput

The engine now handles about 28,000 observations a second, from 2,000 to 16,000 tracks updating
once a second. It handled about 3,900 before.

| Tracks | Before | Engine | Whole run |
|---|---|---|---|
| 2,000 | 3,880/s | 29,400/s | 19,000/s |
| 8,000 | 3,920/s | 27,700/s | 18,100/s |
| 16,000 | not measured | 26,800/s | 17,700/s |

"Whole run" also counts the benchmark decoding and feeding its own data, which the sources
process does in a deployment.

- **One database connection.** The engine keeps one SQLite connection open. It used to open one
  for every decision, and closing it rewrote and deleted the database journal: about 6.5 ms per
  new track, a cap of some 150 new tracks a second.
- **Batched Redis writes.** Within a batch of observations, the engine writes source reports and
  system tracks together in one atomic pipeline, each track once. It used to make about three
  round trips per observation. Publish decisions are still made as each observation arrives.
- **Load scenarios.** `load-2k`, `load-8k` and `load-16k` in `scripts/benchmark/` feed tracks at
  1 Hz for a minute and report observations a second.

### Registry spreadsheet

- **A publish column** (always, never, or empty for automatic) comes after status, so a track
  manager's publish override survives an export and an import.
- **A sheet without the column** leaves every override as it is.

### Documentation

- **Headless running.** The README now describes running OpenTrack headless.

## 0.2.1 (alpha), 2026-09-26

### Tracker profiles

- **What a profile is.** A sensor's tracker settings as a JSON file, with:
  - its sensor kind and platform
  - where the numbers come from
  - the tracker stage it sets
- **Where they live.**
  - `profiles/trackers/` ships with OpenTrack (`OT_PROFILES_DIR`).
  - Imported profiles go to `profiles/trackers` beside the database.
- **In a source's tracker stage** you can load a profile, save the tuned stage as one, import a
  file, or download one. The stage records which profile it came from and shows when it has
  changed since.
- **API.** `GET/POST /tracker-profiles`, `GET/DELETE /tracker-profiles/{name}`. Imports and deletes
  go in the decision log.
- **Benchmark.** `bench run --profile SOURCE=NAME`.
- **The shipped profiles** span the sensor kinds with a tracker stage:
  - GMTI:
    - `gmti-wide-area`: clutter density 2e-7, a balance between noise and real traffic
    - `gmti-high-clutter`: 1e-6, for very noisy data such as the Garden Island recordings
    - `globalhawk-mti`, `lynx-gmti`, `astor-dmti`: nominal starting points
  - Maritime MTI: `lsrs-maritime-mti`
  - Radars: `marine-radar-x`, `coastal-surveillance-radar`, `air-surveillance-radar`
  - Lidar: `lidar-surface`
- **The STANAG 4607 example** uses `gmti-wide-area`. Its old clutter density, 3e-8, made about 1,150
  tracks in 40 minutes on the Garden Island recordings.

### correlation-4

Radar tracks now join their AIS and ADS-B tracks. On the benchmark:

| Scenario | Recall | Precision | GOSPA |
|---|---|---|---|
| AIS + coastal radar | 49% → 78% | 98% → 96% | 11,869 → 7,151 |
| ADS-B + radar | 50% → 90% | 100% | 17,866 → 6,862 |

The changes:

- A slow feed's last report is reused against a sensor's newer ones, weighted and at most every
  10 s.
- Stopped targets are propagated as a slow drift.
- Aircraft get their own velocity spread.
- Views up to 180 s old are compared.
- The same-source window has its own setting.

The new settings are in the correlation form. See [docs/algorithms.md](docs/algorithms.md).

## 0.2.0 (alpha), 2026-09-26

### Plugins

You can now add codecs, trackers and pairing scorers of your own. The full guide is
[docs/plugins.md](docs/plugins.md).

- **Interface.** The plugin interface is `wit/plugin.wit` (`opentrack:plugin@0.1.0`) and covers
  three kinds of plugin:
  - **codec**: turns frames into records.
  - **tracker**: turns plots into tracks, as a source's tracker stage (`"algorithm": "plugin"`).
  - **scorer**: scores whether a report and a system track are the same object. The engine keeps
    its own pairing test, thresholds and decisions. Each merge decision records the plugin, its
    version and what it said.
- **Two ways to run a plugin**, and the engine treats them the same:
  - **WebAssembly components** run in OpenTrack's process, sandboxed. You set each plugin's grants:
    memory, time per call, directories, network addresses and environment. Every stream gets its
    own instance of the plugin.
  - **External programs** serve the same interface over a socket. Use these for Python with numpy
    or Stone Soup, or for a GPU.
- **Settings → Plugins** lets you:
  - add a component or an external plugin's address
  - enable, disable, grant, check and delete plugins
  - see where each plugin is used

  Every role picks up changes within 5 s, and every change goes in the decision log. The tracker
  stage and the correlation settings choose plugins with forms built from each plugin's manifest.
- **Command line.** `opentrack plugin check|add|list`.
- **SDKs** (`sdk/`):
  - **Rust** builds components. Examples: an alpha-beta tracker, a CSV codec and a speed-veto
    scorer.
  - **Python** serves plugins externally, or builds plain Python into components with
    componentize-py. Examples: a domain and course scorer, and a Stone Soup GNN tracker.
- **Built-in plugins.** STANAG 4607 is now a built-in plugin in the same registry.

### Benchmark

- **`opentrack bench`** runs a recorded scenario through the real pipelines and engine, on the
  scenario's own clock and as fast as they go, and publishes nothing.
- **`scripts/benchmark/`** builds and scores 27 scenarios, compares runs, and sweeps settings
  (`--set`) and plugins (`--plugin`). The scenarios are:
  - nine Autoferry lidar and radar recordings, each with and without a vessel feed
  - Garden Island GMTI with the exercise GPS
  - Stone Soup's Solent AIS and OpenSky ADS-B with simulated radars
  - a synthetic case of targets crossing in heavy clutter
- **Scores** cover:
  - GOSPA
  - SIAP-style completeness, false tracks, accuracy and identity
  - track counts
  - correlation precision and recall
- **Data stays out of the repository**, in `$OT_BENCH_DATA`.
- The replay tools moved to `scripts/benchmark/replay/`.

### Engine

- **Scenario clock.** Lifecycle and timers follow the scenario's clock in a replay.
- **Non-blocking reads.** A zero read block now returns at once. Redis checks BLOCK timeouts only
  about every 100 ms, which made fast replays 30 times slower.

### Upgrading

- **Migration 0012** adds the `plugins` table. It replaces an unused placeholder, and runs on
  start.
- **Container builds** now copy `wit/`.

## 0.1.0 (alpha), 2026-09-26

The first alpha has:

- **Sources**:
  - transports: TCP, UDP, HTTP polling, WebSocket, MQTT and file
  - codecs: JSON, XML, Cursor-on-Target and STANAG 4607
  - a pipeline designer with live preview
- **Trackers**: GNN and MHT, each track with an existence probability.
- **Correlation** (correlation-3): covariance-propagated likelihood-ratio pairing, splits, and a
  decision log and track graph.
- **Entities**: one Entity model, with pipeline links in either direction and a registry
  spreadsheet.
- **Track management**: pair, merge, group (battle groups, flights, convoys with 2525C echelon
  symbols), delete and designate.
- **Output**: NATS JetStream in an OTH-GOLD style message.
- **UI**: React on stareSDK.
