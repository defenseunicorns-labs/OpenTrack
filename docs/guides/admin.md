# OpenTrack administrator guide

For the people who install, configure, secure and keep OpenTrack running. The people who work on
the picture day to day have the [operator guide](operator.md). Written for OpenTrack **0.4.7**.

Every section has a short, stable anchor, so the ⓘ tips in the UI and other documents can link to
it (in the app: `#help/admin/<anchor>`). An anchor is the heading's slug, by the rule GitHub uses:
lowercase, every character that is not a letter, digit, space, hyphen or underscore removed, and
spaces turned into hyphens. Where a heading cannot give the anchor wanted, an `<a id="…"></a>` line
just above it sets the anchor instead. Anchors are unique within a guide. Don't rename a heading
without keeping its anchor.

## Overview

OpenTrack is one binary, `opentrack`. Each subcommand runs one **role**:

| Role | What it does | Needs |
|---|---|---|
| `serve` | the REST API (`/api/v1`) and the web UI, on `OT_BIND` (default `0.0.0.0:8090`) | SQLite, Redis, NATS |
| `sources` | every enabled source: transports, decoding, mapping, trackers | SQLite, Redis |
| `engine` | correlation, track lifecycle, track management, groups | SQLite, Redis |
| `writer` | publishes system tracks from the Redis outbox to NATS JetStream | Redis, NATS |
| `link` | exchanges tracks and decisions with other OpenTrack nodes (idle until enabled) | SQLite, Redis, NATS |

`opentrack all` runs all five in one process. That is how most sites run it.

Two stores hold the state:
- **SQLite** holds everything a person decided or configured: sources and their revisions, the
  output schema, the entity registry, correlation and instance settings, accounts, plugins, the
  decision log and the track graph.
- **Redis** holds everything the feeds produce: observation streams, live source and system tracks,
  the outbox, position history and metrics. See [Redis](#redis).

The API sends track management to the engine as commands through Redis and waits up to 10 s for
the answer. So the roles can run in one process or in several.

## Installation

### Requirements

- **Redis** (any recent version). OpenTrack keeps its keys under one prefix (`OT_REDIS_NAMESPACE`,
  default `tms`), so it can share a Redis with other applications.
- **NATS with JetStream**, to publish tracks. Without it, OpenTrack runs, but nothing reaches
  consumers.
- **To build from source:** Rust (stable, see `rust-toolchain.toml`) and Node 22. SAML needs
  `libxmlsec1` (the Docker image has it). A lean build without SAML needs neither: `cargo build
  --release --no-default-features`.

### Docker compose

The `docker-compose.yml` in the repository runs `opentrack all` beside its own Redis, publishing
to an existing NATS (OpenStare's, as a rule):
- **A bridge network.** Only port 8090 is published (`OT_BIND_PORT` picks the host port, for
  example `127.0.0.1:8090` to keep it behind a proxy on the same host). Redis sits on a second,
  internal network that only OpenTrack reaches and that has no route out; it is never published.
- **NATS on the host.** The container reaches the host as `host.docker.internal`
  (`extra_hosts: host-gateway`), so `OT_NATS_URL` defaults to
  `nats://host.docker.internal:4222`. NATS must listen on an address the Docker bridge reaches
  (not only `127.0.0.1`). A NATS elsewhere needs only its URL.
- **Named volumes:** `opentrack-data` at `/data` (the SQLite database and `session.key`) and
  `redis-data` for Redis.
- **Limits:** OpenTrack gets 2 GB of memory, 2 CPUs and 1,024 processes (`OT_MEM_LIMIT`,
  `OT_CPUS`, `OT_PIDS_LIMIT`); 16,000 live tracks at 1 Hz use about one core and 300 MB. Redis
  gets 16 GB and 2 CPUs (`OT_REDIS_MEM_LIMIT`, `OT_REDIS_CPUS`). Redis grows with history and
  the observation window: a history point costs about 130 bytes (2,000 tracks for 12 h at 10 s
  is about 1.1 GB), and `OT_OBS_WINDOW_SECS` at 600 s holds about 4.8 GB at 16,000 reports a
  second. Size `OT_REDIS_MEM_LIMIT` from those, with headroom for Redis' snapshots.
- **Hardening** on both containers: read-only root file system, every capability dropped,
  `no-new-privileges` ([Container hardening](#container-hardening)).

1. Create a `.env` file next to `docker-compose.yml`. Compose refuses to start without it. Put
   your `OT_*` settings and secrets in it (see [Configuration](#configuration)). At least set
   `OT_SITE_CODE`, and `OT_ADMIN_EMAIL` with `OT_ADMIN_PASSWORD`. Leave `OT_BIND` and
   `OT_REDIS_URL` out: the defaults are right for this file.
2. Start it: `docker compose up -d --build`.
3. Open `http://<host>:8090` and sign in.

For a local NATS with JetStream while testing: `docker compose --profile dev-nats up -d`, with
`OT_NATS_URL=nats://nats:4222` in `.env`.

In the image, `OT_SQLITE_PATH` is `/data/opentrack.db`, `OT_UI_DIR` is `/opt/opentrack/ui` and
`OT_PROFILES_DIR` is `/opt/opentrack/profiles/trackers`. The image has no shell: run the
command-line tools as the binary itself, for example
`docker compose exec opentrack opentrack user list`.

#### Upgrading a host-network deployment

Up to 0.4.3 the compose file ran OpenTrack with host networking, a `./data` bind mount and an
external Redis. To move a node to the hardened file:

1. **Back up** `./data` ([SQLite backup](#sqlite-backup)), then stop OpenTrack:
   `docker compose down`.
2. **Check `.env`.** Remove `OT_BIND` (or set it to `0.0.0.0:8090`) and `OT_DATA_DIR`. Remove
   `OT_REDIS_URL` to use the Redis in the compose file. Change a `127.0.0.1` NATS URL to
   `host.docker.internal` (for example `nats://host.docker.internal:4222`), and make sure that
   NATS listens on an address the Docker bridge reaches. Feeds that OpenTrack reaches on
   `127.0.0.1` need the same change.
3. **Create the volumes** without starting anything: `docker compose create`. The data volume is
   `<project>_opentrack-data`, where the project is this directory's name in lower case (or
   `COMPOSE_PROJECT_NAME`); `docker volume ls` shows it.
4. **Copy `./data` into it,** keeping uid 1000 as the owner (the image has no shell, so use
   another image):

   ```sh
   docker run --rm -v "$PWD/data:/from:ro" -v <project>_opentrack-data:/to \
     debian:bookworm-slim sh -c 'cp -a /from/. /to/ && chown -R 1000:1000 /to'
   ```
5. **Redis (optional).** The new Redis starts empty. OpenTrack rebuilds the picture from the
   feeds, and tracks take new numbers (never ones already issued; [Restore](#restore)). To keep
   the live picture instead, save the old Redis and copy the file in before starting:

   ```sh
   redis-cli -h 127.0.0.1 -p 6379 --rdb dump.rdb
   docker run --rm -v "$PWD/dump.rdb:/dump.rdb:ro" -v <project>_redis-data:/to \
     debian:bookworm-slim sh -c 'cp /dump.rdb /to/dump.rdb && chown 999:1000 /to/dump.rdb'
   ```

   Or keep the external Redis: set `OT_REDIS_URL=redis://host.docker.internal:6379` (Redis must
   listen on an address the bridge reaches, with a password or TLS), and stop the bundled one
   from starting as in the host-networking override below.
6. **Start it:** `docker compose up -d --build`. Check **Overview → System status**, then move
   `./data` somewhere safe; nothing reads it any more.

**Keeping host networking.** A site that must stay on the host network (an accepted exception;
[hardening](../security/hardening.md)) keeps the rest of the file with a
`docker-compose.override.yml` next to it, which compose reads by itself:

```yaml
services:
  opentrack:
    network_mode: host
    networks: !reset []
    ports: !reset []
    extra_hosts: !reset []
    depends_on: !reset {}
    environment:
      OT_REDIS_URL: ${OT_REDIS_URL:-redis://127.0.0.1:6379}
      OT_NATS_URL: ${OT_NATS_URL:-nats://127.0.0.1:4222}
  redis:
    profiles: ["bundled-redis"]   # never started; the host's Redis is used
```

To keep the `./data` directory as well, add `volumes: !override ["./data:/data"]` under
`opentrack`.

### From source

```sh
cargo build --release
(cd ui && npm ci && npm run build)
./target/release/opentrack all
```

Run it from the repository root, or set `OT_UI_DIR` and `OT_PROFILES_DIR`: their defaults
(`ui/dist`, `profiles/trackers`) are relative to the working directory. Without a built UI,
`serve` and `all` serve the API alone.

To check the whole pipeline end to end, push a synthetic track through it, verify it arrives in
the NATS stream, then retire it: `opentrack synthetic --verify --retire`.

### First start

On its first start OpenTrack:
- creates and migrates the SQLite database at `OT_SQLITE_PATH` (and its directory);
- makes the session signing key, `session.key`, beside the database, unless `OT_SESSION_SECRET` is
  set;
- makes the first admin account (see [First admin](#first-admin));
- creates the JetStream stream `OT_NATS_STREAM` if it is missing. It never changes an existing
  stream.

Then sign in, and in **Settings**:
1. set the site name and, if needed, the classification banner ([Banners](#banners));
2. add accounts ([Users](#users)) and set up single sign-on ([Sign-in](#sign-in));
3. add sources (the **Sources** tab), and publish an output schema (the **Schema** tab).

### Headless

OpenTrack needs no UI and no operator. Everything the UI does is a REST call, and configuration can
also come from files:
- source specs as JSON (`docs/examples/` has several), posted to `/api/v1/sources`;
- tracker profiles as JSON files in `profiles/trackers/`;
- plugins with `opentrack plugin add`;
- accounts with `opentrack user` ([User commands](#user-commands)).
- or everything at once, from another node's export: `opentrack config import <file>`
  ([Rebuild a node](#rebuild-a-node-from-a-configuration-export)).

A node that only collects and correlates needs `sources` and `engine`, plus `writer` to publish.
It can start from a database prepared elsewhere (`OT_SQLITE_PATH`). Give every node its own
`OT_SITE_CODE`, so track numbers never clash.

## Command line

`opentrack [common options] <command> [options]`. The common options (the database, Redis, NATS,
site code and the rest of [Core settings](#core-settings)) come before the command, or from their
`OT_*` variables. `opentrack --help` and `opentrack <command> --help` list every option.

### Server roles

| Command | What it runs |
|---|---|
| `migrate` | Creates or upgrades the SQLite database, prints its schema version, and exits. |
| `serve` | The control plane: REST API and UI. |
| `sources` | Every enabled source. |
| `engine` | The engine. Run exactly one per Redis namespace: it owns the live picture. |
| `writer` | The writer. Several may run; give each its own `OT_WRITER_CONSUMER`. |
| `cot` | The TAK output: the published tracks as Cursor-on-Target to the outputs Settings → TAK output configures ([TAK output](#tak-output)). Idle until one is on. Run one per Redis namespace. |
| `link` | This node's side of the link to other nodes ([Multi-node](#multi-node)). |
| `bridge` | Carries sync messages between nodes' NATS servers, for server sites with no networking package of their own: `opentrack bridge --node nats://a:4222 --node nats://b:4222`. It can also drop, delay, duplicate, cap and partition messages, for tests (`--loss`, `--delay-ms`, `--jitter-ms`, `--duplicate`, `--rate-kbps`, `--partition`). TLS and credentials: `--nats-*` / `OT_BRIDGE_NATS_*` ([The bridge's NATS credentials and TLS](#the-bridges-nats-credentials-and-tls)). |
| `all` | `serve`, `sources`, `engine`, `writer`, `cot` and `link` in one process. It migrates the database once before they start. |

### Other commands

| Command | What it does |
|---|---|
| `synthetic` | Pushes synthetic tracks through the pipeline: `--count`, `--lat`, `--lon`, `--move-secs`; `--verify` waits until each is in the NATS stream, `--retire` retires them afterwards. To test without touching operational subjects, give it and the writer their own `OT_NATS_STREAM`, `OT_NATS_TRACKS_SUBJECT` and `OT_REDIS_NAMESPACE`. |
| `bench` | Runs a recorded scenario through the real pipelines and engine as fast as they go and writes the results (`opentrack bench <scenario dir> --out <dir>`). Publishes nothing. See `scripts/benchmark/README.md`. |
| `config export [--out <file>]` / `config import <file>` / `config status` | Writes the whole configuration (secrets included) to standard output or a file; rebuilds an empty node from one; says whether this node is empty. See [Configuration export](#configuration-export). |
| `retire <uid>` | Retires one system track: records the decision (`--reason`), closes its links in the track graph and queues its delete for the writer. Takes `OTK000000042` or `tms-OTK000000042`. |

### Plugin commands

- `opentrack plugin check <file.wasm | address> [--grants '<json>'] [--secret-file <path>]` loads a plugin, opens each
  kind it provides with its default options, and prints its manifest and what worked. It changes
  nothing and exits non-zero when something failed.
- `opentrack plugin add <file.wasm | address> [--grants '<json>'] [--secret-file <path>] [--replace]` adds it, as
  Settings → General → Plugins does. `--replace` installs a new build of a plugin that is already added.
- `opentrack plugin list` lists the plugins added and whether each is enabled.

An address is `host:port`, `tcp://host:port` or `unix:/path`. Grants are JSON, for example
`'{"memory_mb": 512}'`. An external plugin's secret comes from `--secret-file` or
`OT_PLUGIN_SECRET`, never the command line. See [Plugins](#plugins).

### User commands

For a node without the UI, or when no admin can sign in. Each change goes in the decision log as
the actor `cli`. The commands open the database directly, so they work while the server runs.

| Command | What it does |
|---|---|
| `opentrack user list` | Every account: email, role, origin (`local` or `saml`), active or off, and whether it has a password. |
| `opentrack user add <email> --role <role> [--name <name>] [--password-stdin] [--temporary]` | Adds an account (`--temporary`: turned off 72 hours after it is made). With `--password-stdin` the password is the first line of standard input (it must meet the [password policy](#passwords)). Without one, the account can only use single sign-on or an API token. |
| `opentrack user role <email> <role>` | Changes the role: `viewer`, `track_manager` or `admin`. |
| `opentrack user passwd <email>` | Sets the password from standard input. The account's sessions and API tokens end. |
| `opentrack user disable <email>` / `enable <email>` | Turns the account off (its sessions and tokens stop working at once) or on. |
| `opentrack user token <email> --name <what for> [--days 365]` | Prints a new API token that acts as the account. It must be signed with the server's key: run it with the same `OT_SESSION_SECRET`, or next to the same `session.key`. |

For example, from a file only you can read:

```sh
opentrack user passwd admin@example.org < /secure/new-password.txt
```

## Configuration

Every setting is a command-line option or an `OT_*` environment variable. With Docker, put them in
`.env`. Settings people change while OpenTrack runs (sources, schema, correlation, banners,
accounts, sign-in) are not here: they live in the database and are changed in the UI or the API.

An empty variable counts as unset for the sign-in and TLS settings below, because Compose passes
unset variables as empty ones.

### Core settings

For every role.

| Variable | Default | |
|---|---|---|
| `OT_SQLITE_PATH` | `data/opentrack.db` | The SQLite database. Created and migrated on start, with its directory. Other files live beside it ([Data directory](#data-directory)). |
| `OT_REDIS_URL` | `redis://127.0.0.1:6379` | |
| `OT_REDIS_NAMESPACE` | `tms` | Prefix of every Redis key OpenTrack owns. |
| `OT_REDIS_CA` | | TLS to Redis (with a `rediss://` URL): trust this CA (PEM) instead of the system's roots. |
| `OT_REDIS_CERT`, `OT_REDIS_KEY` | | Mutual TLS to Redis: this client certificate and key (PEM). |
| `OT_SITE_CODE` | `OTK` | The GOLD site code that starts every track number (3 characters, A–Z and 0–9). Fixed for a deployment: changing it changes every new track number. Every node needs its own. |
| `OT_PROFILES_DIR` | `profiles/trackers` | Tracker profiles shipped with OpenTrack (read only). Profiles imported in the UI go to `profiles/trackers` beside the database instead. |
| `OT_OBS_WINDOW_SECS` | `600` | How long each source's observation stream keeps reports, in seconds (at least 10). The engine can be down this long without losing any. Redis holds about 500 bytes a report: 16,000 reports a second for 600 s is about 4.8 GB. |

### NATS output

For `serve`, `writer`, `link` and `synthetic`.

| Variable | Default | |
|---|---|---|
| `OT_NATS_URL` | `nats://127.0.0.1:4222` | NATS server(s), comma separated. |
| `OT_NATS_CREDS` | | A credentials file (JWT and NKey). |
| `OT_NATS_TOKEN` | | A token. |
| `OT_NATS_USER`, `OT_NATS_PASSWORD` | | A user and password. |
| `OT_NATS_CA` | | TLS to NATS: trust this CA (PEM), and refuse a connection without TLS. |
| `OT_NATS_CERT`, `OT_NATS_KEY` | | Mutual TLS to NATS: this client certificate and key (PEM). |
| `OT_NATS_STREAM` | `TRACKS` | The JetStream stream. Created if missing, never modified. |
| `OT_NATS_TRACKS_SUBJECT` | `tracks` | Subject prefix: each track is published on `<prefix>.tms-<UID>`. |
| `OT_NATS_MAX_AGE_HOURS` | `24` | Message age limit of a stream OpenTrack creates. Live tracks are republished long before this. |

The message consumers receive is described in [docs/nats-output.md](../nats-output.md).

### Control plane

For `serve` (and `all`).

| Variable | Default | |
|---|---|---|
| `OT_BIND` | `0.0.0.0:8090` | Address the API and UI listen on. |
| `OT_ADMIN_BIND` | | A second address, for administration only: the admin routes are served there and nowhere else ([Admin listener](#admin-listener)). |
| `OT_UI_DIR` | `ui/dist` | The built UI, served at `/`. Skipped when it has no `index.html`. |
| `OT_AUTH` | `on` | `off` turns sign-in off: every caller is an admin. Development only ([Sign-in turned off](#sign-in-turned-off)). |
| `OT_ADMIN_EMAIL`, `OT_ADMIN_PASSWORD` | | The first admin account, made only while there are no accounts ([First admin](#first-admin)). |
| `OT_SESSION_SECRET` | `session.key` beside the database | The key sessions and API tokens are signed with, at least 32 characters. Unset, one is made on first start and kept in `session.key`. Changing it signs everyone out and voids every API token. |
| `OT_PUBLIC_URL` | | Where browsers reach OpenTrack, such as `https://opentrack.example.org`. SAML needs it. An `https://` address also marks cookies `Secure` and sends HSTS. Behind a proxy that rewrites `Host` and sends no `X-Forwarded-Host`, set it so the page's changes pass the cross-site check ([Security](#security)). |
| `OT_PUBLIC_TLS` | off | `1`: a proxy in front of OpenTrack ends TLS, so cookies are `Secure` and HSTS is sent. |

### TLS

For `serve`. With a certificate, the control plane serves HTTPS only, and session cookies are
marked `Secure`.

| Variable | |
|---|---|
| `OT_TLS_CERT` | The server certificate (PEM, with its chain). Needs `OT_TLS_KEY`. |
| `OT_TLS_KEY` | Its private key (PEM). |
| `OT_TLS_CLIENT_CA` | Accept client certificates this CA (PEM) signed, as the accounts [Client certificates](#client-certificates) maps them to. A client certificate is optional: browsers without one still sign in with a password or single sign-on. |
| `OT_TLS_CLIENT_CRL` | Certificate revocation lists for client certificates: PEM or DER files, or directories of them, comma separated. Used when OCSP gives no answer: a revoked certificate, one no list covers, or one whose issuer's list is past its next update is then refused. Reloaded within a minute of a change. |
| `OT_TLS_CLIENT_OCSP_URL` | The OCSP responder (`http://` or `https://`) to ask about every client certificate, instead of the one each certificate names in its authority information access extension. Needs `OT_TLS_CLIENT_CA`. See [Client certificates](#client-certificates). |
| `OT_ADMIN_TLS_CERT`, `OT_ADMIN_TLS_KEY` | The [admin listener](#admin-listener)'s own certificate and key, together. Unset: it uses `OT_TLS_CERT` and `OT_TLS_KEY`. |
| `OT_ADMIN_TLS_CLIENT_CA` | The admin listener's own client CA. Unset: `OT_TLS_CLIENT_CA`. The revocation lists (`OT_TLS_CLIENT_CRL`) apply to both listeners. |

#### Admin listener

With `OT_ADMIN_BIND` (for example `10.20.0.5:8091`, an address on the management network, or
`127.0.0.1:8091`), OpenTrack listens there as well as on `OT_BIND`. Every admin route (accounts,
tokens, security and sign-in settings, sources, settings, plugins, configuration export and
import, the audit record, and every other change that needs the admin role) is served **only** on
the admin listener. On `OT_BIND` an admin asking for one gets `404` ("not served on this
address"): that listener has no admin interface. Everyone else is refused there as anywhere
(`401` without a sign-in, `403` without the role). The admin listener serves the whole UI and API
with the same sign-in; on the main one the UI hides the admin areas (`/auth/me` says
`admin_api: false`).

It takes the main listener's TLS unless `OT_ADMIN_TLS_*` set its own. `OT_ADMIN_BIND` can't be
`OT_BIND`, and `OT_ADMIN_TLS_*` need it; OpenTrack refuses to start otherwise. Publish the port
only to the management network, and firewall it ([hardening](../security/hardening.md)). Unset,
nothing changes. `opentrack health` still checks `OT_BIND`.

TLS for feeds is set per source, in the source's transport (see the README, "Sources and
pipelines").

### Engine and writer

| Variable | Default | |
|---|---|---|
| `OT_ENGINE_CONSUMER` | `engine-1` | The engine's consumer name on the observation streams. |
| `OT_CONFIRM_AFTER` | `3` | Reports a new system track needs before it is confirmed (at least 1). A source's own **Confirm after** (in its publish stage) overrides it for that source's tracks. |
| `OT_DROP_AFTER_HOURS` | `6` | Hours without a report before a system track is dropped and deleted downstream. |
| `OT_CORRELATION` | `kinematics-metadata` | How tracks with no shared identifier pair, until someone saves correlation settings: `identifiers` (never), `kinematics`, or `kinematics-metadata` (kinematics, vetoed by conflicting identifiers or domains). Once correlation settings are saved in the Correlation tab, the saved ones win and this is ignored; the engine logs a warning at start when it is set and the saved approach differs. |
| `OT_WRITER_CONSUMER` | `writer-1` | The writer's consumer name; unique per writer. |
| `OT_WRITE_MIN_INTERVAL_SECS` | `5` | At most one publication of a track this often. Significant changes (identity, classification, state) go at once. |

A track is marked lost when its sources stop reporting: after 60 s for air and space tracks,
30 minutes for subsurface, 15 minutes for the rest. It is dropped after `OT_DROP_AFTER_HOURS`.

### TAK output role

For `cot` (and `all`). The outputs themselves are set in Settings → TAK output ([TAK output](#tak-output)).

| Variable | Default | |
|---|---|---|
| `OT_COT_CONSUMER` | `cot-1` | The `cot` role's consumer name in its group (`track-cot`) on the outbox. Run one `cot` role per Redis namespace: two would split the tracks between them. |
| `OT_COT_MIN_INTERVAL_SECS` | `2` | At most one event of a track this often. Identity and classification changes go at once. |

### Multi-node sync

For `link`. See [Multi-node](#multi-node).

| Variable | Default | |
|---|---|---|
| `OT_SYNC_PREFIX` | `ot.sync` | Subject prefix of the sync boundary on this node's NATS: `<prefix>.out.<kind>` and `<prefix>.in.<kind>`. |
| `OT_SYNC_SUMMARY_SECS` | `5` | Seconds between the summaries that let nodes find the decisions they missed. |

### Logging

| Variable | Default | |
|---|---|---|
| `OT_LOG` | `info` | Log filter, in `tracing` syntax: `warn`, `debug`, or per module, such as `info,opentrack=debug,ot_source=debug`. |
| `OT_LOG_FORMAT` | text | `json` writes one JSON object a line, for log collectors. |

Every API request is logged at `info` with target `access`: the account, method, path, status,
client address, user agent, referrer, `X-Forwarded-For` and duration. It goes wherever the logs
go (standard output and [OpenTelemetry](#opentelemetry)). `OT_LOG=info,access=warn` turns it
off for both, which an accredited deployment shouldn't do: it is part of the audit trail.

The server roles log to standard output. The one-off commands (`migrate`, `user`, `config`,
`plugin`, `retire`, `bench`, `synthetic`) log to standard error, so their output on standard
output can be piped or redirected on its own.

### OpenTelemetry

OpenTrack sends its logs (the audit record included), traces and metrics over OpenTelemetry
(OTLP) to the collector your deployment runs; the collector and the back end behind it (SIEM, log
store, metrics and tracing systems) keep and search them. Export is off until an endpoint is set,
and standard output keeps every log line either way. Set the standard `OTEL_*` variables on every
role (with `opentrack all`, on the one process):

| Variable | Default | |
|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | unset | The collector, such as `https://otel-collector:4318` (HTTP) or `https://otel-collector:4317` (gRPC). Setting it turns export on. `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT`, `..._TRACES_ENDPOINT` and `..._METRICS_ENDPOINT` set one signal's (an HTTP one is the full URL, with `/v1/logs` and so on). |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | `http/protobuf` | `grpc` or `http/protobuf` (per signal too: `OTEL_EXPORTER_OTLP_LOGS_PROTOCOL` and so on). |
| `OTEL_EXPORTER_OTLP_CERTIFICATE` | system roots | PEM file of the CA that signed the collector's certificate. |
| `OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE`, `OTEL_EXPORTER_OTLP_CLIENT_KEY` | unset | PEM client certificate and key, for mutual TLS. |
| `OTEL_EXPORTER_OTLP_HEADERS` | unset | Headers on every export, such as `authorization=Bearer …`. |
| `OTEL_EXPORTER_OTLP_TIMEOUT`, `OTEL_EXPORTER_OTLP_COMPRESSION` | 10 s, none | Per export; `gzip` compresses. |
| `OTEL_SERVICE_NAME`, `OTEL_RESOURCE_ATTRIBUTES` | `opentrack` | The resource. OpenTrack adds `service.version`, `service.instance.id` (host and process), `opentrack.node_id` and `opentrack.role`. |
| `OTEL_TRACES_SAMPLER`, `OTEL_TRACES_SAMPLER_ARG` | one trace in ten | As the specification says; unset, `parentbased_traceidratio` at 0.1. |
| `OTEL_METRIC_EXPORT_INTERVAL` | 60000 | Milliseconds between metric exports. |
| `OTEL_LOGS_EXPORTER`, `OTEL_TRACES_EXPORTER`, `OTEL_METRICS_EXPORTER` | `otlp` | `none` leaves that signal out. |
| `OTEL_SDK_DISABLED` | `false` | `true` turns all export off. |

TLS to the collector (`https://` endpoints) runs on the FIPS module, like every other connection.

**What is sent:**
- **Logs:** every log line at or above `OT_LOG`, with its fields as attributes and its target as
  the instrumentation scope. Audit records have the event name **`audit.record`** and scope
  `audit` ([Audit record](#audit-record)): route them to your SIEM on either. The exporters' own
  transport logs, and OpenTrack's export warnings, are not sent (a failing collector would be fed
  its own failures).
- **Traces:** API requests (`request`), each source's frame processing and writes
  (`source.process`, `source.write`), the engine's correlation batches (`engine.batch`) and the
  writer's publishes (`writer.flush`).
- **Metrics:** everything the Overview charts, as `opentrack.<name>`, with the source or role as
  `opentrack.component` and a TAK output as `opentrack.output`: counters such as
  `opentrack.frames`, `opentrack.observations`, `opentrack.sent`, and gauges such as
  `opentrack.tracks`, `opentrack.outbox_lag`, `opentrack.rss_bytes`. The Overview keeps its own
  history in Redis as before.

**When the collector can't be reached**, OpenTrack keeps working. Each role logs `OpenTelemetry
export failing` once, on standard output, and again when it recovers; the **Telemetry** row of
**Overview → System status** turns red with the error (it goes green within about 10 seconds of
the collector taking connections again). Records that can't be sent are dropped from the export,
not held without bound: standard output still has every log line, and the audit table every
audit record, so fill the SIEM's gap from `GET /api/v1/audit?from_ms=…&to_ms=…`.

A minimal collector that takes both protocols and forwards to your back end:

```yaml
receivers:
  otlp:
    protocols:
      grpc: { endpoint: 0.0.0.0:4317 }
      http: { endpoint: 0.0.0.0:4318 }
processors:
  batch: {}
exporters:
  otlphttp: { endpoint: https://siem.example.mil:4318 }
service:
  pipelines:
    logs: { receivers: [otlp], processors: [batch], exporters: [otlphttp] }
    traces: { receivers: [otlp], processors: [batch], exporters: [otlphttp] }
    metrics: { receivers: [otlp], processors: [batch], exporters: [otlphttp] }
```

### Test-only variables

These are read only by the test suites and benchmark tools, never by a running server:
`OT_TEST_REDIS_URL`, `OT_TEST_NATS_URL`, `OT_TEST_MQTT_URL`, `OT_GMTI_SAMPLES`, `OT_BENCH_DATA`,
`OT_BENCH_GMTI`, `OT_BENCH_GPS`, `OT_REPLAY_TRACE` and `OT_ESM_NAIVE`. `OT_DATA_DIR` is read
only by `docker-compose.yml` (the host directory mounted at `/data`).

## Ports, protocols and services

Every port and protocol OpenTrack listens on or connects to, for PPSM registration and firewall
rules. Only the control plane listens by default; everything else is there only once it is
configured. *Inbound* means OpenTrack listens; *outbound* means it connects. All TLS runs on the
FIPS module ([FIPS cryptography](#fips-cryptography)).

| Service | Protocol | Direction | Default | Purpose | TLS and authentication |
|---|---|---|---|---|---|
| Control plane | TCP; HTTP/1.1 and HTTP/2, HTTPS with a certificate | Inbound | `OT_BIND`, `0.0.0.0:8090` | The web UI, the REST API (`/api/v1`), the SAML assertion consumer, `/healthz` | `OT_TLS_CERT`/`OT_TLS_KEY`, or a TLS proxy (`OT_PUBLIC_TLS`); client certificates with `OT_TLS_CLIENT_CA`, checked by OCSP and `OT_TLS_CLIENT_CRL`. Sign-in: session, API token, client certificate or OpenStare ([Sign-in](#sign-in)) |
| OCSP responder | TCP, HTTP (or HTTPS) | Outbound | The client certificate's authority information access URL, or `OT_TLS_CLIENT_OCSP_URL` | The revocation status of a client certificate presented to the control plane (with `OT_TLS_CLIENT_CA`) | OCSP requests and responses (RFC 6960); answers are signed by the CA or its delegated responder, so plain HTTP is usual. 5 s limit, no redirects |
| Admin listener | TCP; HTTP/1.1 and HTTP/2, HTTPS with a certificate | Inbound | `OT_ADMIN_BIND`, none | The admin API routes, and the web UI and the rest of the API ([Admin listener](#admin-listener)); only here while it is set | As the control plane, or `OT_ADMIN_TLS_CERT`/`OT_ADMIN_TLS_KEY` and `OT_ADMIN_TLS_CLIENT_CA`; the same sign-in |
| Redis | TCP, RESP | Outbound | `redis://127.0.0.1:6379` | Observation streams, the live picture, the outbox, commands between roles | `rediss://`, `OT_REDIS_CA`, mutual TLS with `OT_REDIS_CERT`/`OT_REDIS_KEY`; a password in the URL |
| NATS | TCP, NATS | Outbound | `nats://127.0.0.1:4222` | Tracks (`tracks.>`, JetStream), contacts (`contacts.>`) and raw output (`opentrack.raw.<source>`); the multi-node sync boundary (`<OT_SYNC_PREFIX>.out.*` and `.in.*`, prefix `ot.sync`) | `tls://`, `OT_NATS_CA`, mutual TLS with `OT_NATS_CERT`/`OT_NATS_KEY`; `.creds`, a token, or a user and password |
| OpenTelemetry | TCP; OTLP over gRPC (4317) or HTTP (4318) | Outbound | Off | Logs (the audit record included), traces and metrics to the collector ([OpenTelemetry](#opentelemetry)) | `https://` endpoint, `OTEL_EXPORTER_OTLP_CERTIFICATE`, mutual TLS, headers |
| TAK Server output | TCP, CoT XML | Outbound | None configured | Tracks to a TAK Server's streaming input ([TAK output](#tak-output)) | TLS (8089) with a client certificate; 8087 is plaintext |
| TAK multicast output | UDP, CoT XML, IPv4 or IPv6 | Outbound | Group `239.2.3.1`, port `6969`, TTL 1 (IPv6: hop limit) | Tracks to ATAK and WinTAK on the network | None: always plaintext |
| TAK listening output | TCP, CoT XML | Inbound | None configured (**Listen on**, such as `0.0.0.0:8089`) | ATAK and WinTAK connect as to a TAK Server | TLS, with an optional client CA and revocation lists |
| Source `tcp_server` | TCP | Inbound | None: the source's `bind` | A feed that connects to OpenTrack | TLS; mutual TLS with `client_ca_file` and revocation lists. Required, or the source's `unauthenticated: accepted` risk acceptance |
| Source `udp` | UDP, optionally joining an IPv4 or IPv6 multicast group | Inbound | None: the source's `bind` | A feed sent as datagrams | None: runs only with the source's `unauthenticated: accepted` risk acceptance |
| Source `grpc_server` | TCP, gRPC over HTTP/2 | Inbound | None: the source's `bind` | Producers' gRPC calls ([docs/protobuf-grpc.md](../protobuf-grpc.md)) | TLS; mutual TLS with `client_ca_file`; a bearer `token`. One of the two is required, or the source's `unauthenticated: accepted` risk acceptance |
| Source `tcp_client` | TCP | Outbound | None: the source's `host` and `port` | A feed OpenTrack connects to | TLS, client certificate |
| Source `http_poll` | TCP, HTTP or HTTPS | Outbound | None: the source's `url` | A polled feed | `https://`, CA, client certificate; headers (such as a token) |
| Source `websocket` | TCP, WebSocket (`ws://`, `wss://`) | Outbound | None: the source's `url` | A streamed feed | `wss://`, CA, client certificate; headers |
| Source `mqtt` | TCP, MQTT 3.1.1 (1883; 8883 for `mqtts://`) | Outbound | None: the source's `url` | Subscribed topics | `mqtts://`, CA, client certificate; user and password |
| Source `grpc_client` | TCP, gRPC over HTTP/2 | Outbound | None: the source's `url` | A producer's streaming method | `https://`, CA, client certificate; metadata (such as a bearer token) |
| Source `file` | None | | | Recorded data, read only under the data directory | |
| SAML identity provider | HTTPS, through the browser | None from the server: browsers go to the identity provider and post back to the control plane | Off | Single sign-on ([SAML](#saml)) | The identity provider's signature on the response |
| OpenStare sign-in check | TCP, HTTP or HTTPS | Outbound | Off (API URL `http://127.0.0.1:3001`) | Asks OpenStare's `/api/auth/me` who a session or token is ([OpenStare sign-in](#openstare-sign-in)) | `https://` when the API URL is; the user's own OpenStare cookie or token |
| Basemap tiles | TCP, HTTP or HTTPS | Outbound | Off | Map tiles, fetched for the browsers ([Basemap tiles](#basemap-tiles)) | An `https://` URL; a key in the query if the tile server takes one |
| External plugins | TCP (`host:port`) or a Unix socket | Outbound | None added | Codec, tracker and scorer calls as JSON lines ([Plugins](#plugins)) | Mutual HMAC-SHA256 challenge and response with a shared secret on every connection (none needed on a Unix socket under the data directory); no TLS: keep them on the same host or a trusted network |
| WebAssembly plugins | Whatever the plugin opens | Outbound | No network | Only the `host:port` addresses in the plugin's **network** grant | The plugin's own |
| Multi-node sync | NATS subjects (above) | Through NATS | Idle until **Share the picture** is on | Tracks and decisions between nodes ([Multi-node](#multi-node)) | Every message signed with the sender's Ed25519 key and checked against the key pinned for it; stale and replayed messages refused ([Signed sync messages](#signed-sync-messages), [docs/sync-icd.md](../sync-icd.md)) |
| `opentrack bridge` | TCP, NATS | Outbound, to each `--node` | Not run by `all` | Carries sync subjects between server sites' NATS servers | TLS (`tls://`, `OT_BRIDGE_NATS_CA`), mutual TLS (`OT_BRIDGE_NATS_CERT`/`OT_BRIDGE_NATS_KEY`), `.creds`, user and password or token, per node if they differ ([The bridge's NATS credentials and TLS](#the-bridges-nats-credentials-and-tls)) |

`opentrack health`, the image's health check, connects to `OT_BIND` on the same host. The
command-line tools open the database directly and use no port.

## Data directory

The directory that holds the database (`data/` from source, `/data` in Docker):

| File | What it is |
|---|---|
| `opentrack.db` | The SQLite database: sources and their revisions, the output schema, the registry and its revisions, correlation and instance settings, sign-in settings, accounts, API token records, plugins (WebAssembly files included), groups, correlation suggestions, the decision log, the track graph and the UID sequence. |
| `opentrack.db-wal`, `opentrack.db-shm` | SQLite's write-ahead log and its index, while the database is open. They are part of the database: never delete them while OpenTrack runs, and never copy the `.db` file alone while it runs ([SQLite backup](#sqlite-backup)). |
| `session.key` | The key sessions and API tokens are signed with, when `OT_SESSION_SECRET` is not set. Readable by its owner only. Lose it and everyone is signed out, and every API token stops working. |
| `sync.key` | This node's Ed25519 key for signing sync messages (PKCS #8, base64), made on first start. Readable by its owner only. Lose it and the node makes a new one, which every other node must pin ([Signed sync messages](#signed-sync-messages)). |
| `initial-admin.txt` | The first admin's made-up password, when no `OT_ADMIN_*` was given ([First admin](#first-admin)). Delete it once you've changed that password. |
| `profiles/trackers/` | Tracker profiles imported or saved in the UI. |

### Redis

Everything under `<namespace>:` (`tms:` by default):

| Keys | What they hold |
|---|---|
| `tms:obs:<source>` | Each source's observation stream, kept for `OT_OBS_WINDOW_SECS`. |
| `tms:src:…`, `tms:sys:<uid>` | Live source tracks and system tracks: the picture itself. |
| `tms:thist:<uid>` | Each system track's position history ([Instance settings](#settings-tab)). |
| `tms:out`, `tms:out:ctl` | The writer's outbox: updates, and the deletes and history-point deletions (never trimmed). |
| `tms:cmd`, `tms:cmdres:<id>` | Commands from the API to the engine, and the answers. |
| `tms:cache:…` | Cached static data (AIS static reports) for the static join. |
| `tms:status:<source>`, `tms:metrics:…` | Source status, and per-minute metrics (kept for about a week). |
| `tms:sync:…`, `tms:contacts:out` | Messages for other nodes, and bearings no track took, for the writer. |

Redis holds nothing a person decided; it is all rebuilt from the feeds. But the live picture and
its track numbers are there. Lose Redis, and tracks form again under new numbers (never ones
already issued), and the engine publishes deletes for this node's tracks left in the NATS stream.
See [Restore](#restore).

## Users

### Roles

Each role includes the ones before it. The server checks the role on every call, from the account
as it is now, so a role change or a deactivation takes effect at once.

| Role | What it may do |
|---|---|
| `viewer` | See everything: tracks, sources, the registry, the schema, correlation, settings and metrics. Export tracks and the registry. Change nothing but their own password. Viewers cannot see accounts, API tokens, sign-in settings or the configuration export. |
| `track_manager` | Also manage the picture: pair, unpair, merge, split, do-not-pair, delete, group, designate tracks (edit and pin entities), accept and reject correlation suggestions, undo those decisions, delete history points, and edit and import the registry. |
| `admin` | Also configure OpenTrack: sources, the output schema, correlation settings, instance settings and banners, plugins, tracker profiles, nodes, accounts, API tokens, sign-in, the configuration export and purge. |

In the API: reading needs `viewer`; changes under `/tracks/`, `/groups`, `/registry`,
`/correlation/suggestions/`, `/decisions/` and `/history/` need `track_manager`; every other
change needs `admin`, as do reads under `/auth/` (other than your own profile), `/export/config`,
`/import/config`, `/probe` and `/audit`.

Every change records the account that made it in the decision log.

### First admin

When the database has no accounts, `serve` makes an admin:
- from `OT_ADMIN_EMAIL` and `OT_ADMIN_PASSWORD` (it must meet the [password policy](#passwords), or the server refuses to
  start); or, without them,
- `admin@opentrack.local`, with a made-up password written to `initial-admin.txt` beside the
  database. The log says where.

Sign in with it, change the password (the account menu, top right), and delete
`initial-admin.txt`. Once any account exists, `OT_ADMIN_*` is ignored: it never resets a
password.

### Adding accounts

In **Settings → Users**, **Add account**: email, name, role, and a password that meets the
[password policy](#password-policy). Leave the password empty for an account that signs in only with single sign-on.
Accounts that SAML makes appear here on their first sign-on, with the origin `saml`. From the
command line: `opentrack user add`.

**Temporary** (or `--temporary` on the command line) makes a temporary or emergency account: it
is turned off 72 hours after it is made (fixed), with its sessions and API tokens, and the audit
record says so (`account_disabled`, reason `expired`). Its row shows **temporary**, then
**expired**. Turning it on again gives it another 72 hours; the infinity button makes it
permanent.

### Disabling accounts

In the Users table:
- **Role** changes at once.
- **Active** off: the account can't sign in, its sessions end and its API tokens are revoked for
  good. Turned on again, it can sign in; issue it new API tokens.
- **Delete** removes the account and its API tokens. The decision log keeps what it did.

OpenTrack refuses any change that would leave no active admin, and won't delete your own account.
The table doesn't let you turn your own account off.

A SAML account's role comes from the identity provider: it is set again at every sign-on, so change
it in the provider's mapping, not here.

### Passwords

- **Your own:** the account menu (top right) → **Change password**. Your other sessions end. It
  is offered only to accounts that signed in with a password.
- **Someone else's:** the key button on their row. **Set password** sets a temporary one, which the
  user must change at their next sign-in. **Remove password** makes the account single sign-on
  only. Either way the account's sessions **and API tokens** end. Remember this before resetting
  a service account's password.
- **From the command line:** `opentrack user passwd <email>`, reading the new password from
  standard input. It is temporary too.

Every password must meet the [password policy](#password-policy).

Passwords are hashed with PBKDF2-HMAC-SHA256 in the FIPS module (see
[Security hardening](#security-hardening)). Hashes made before 0.4.0 (Argon2id) still work, and
are replaced at the account's next sign-in. Sign-in attempts are limited per client address: 5 at
once, then one a second.

### API tokens

For scripts and services. A token acts as an account, with that account's role, until it expires
or is revoked.

1. **Settings → API tokens → New token**. Name it after what uses it, pick the account it acts as,
   and how many days it lasts (1 hour to 10 years; default 365).
2. Copy it. It is shown once.
3. Send it as `Authorization: Bearer <token>`.

Revoke it with the bin on its row. A token also stops working when its account is turned off,
deleted, signed out everywhere, or has its password set, and when the session key changes.

A dedicated account per service, with the least role it needs (`viewer` for a monitor), keeps
tokens independent of people's passwords.

### Revoking sessions

To sign an account out everywhere, use **Sign out everywhere** (the door button on its row). Every
session and API token it holds ends now; it can sign in again. From the command line, `opentrack
user passwd` also ends them, and so does turning the account off (its API tokens don't come back when it is turned on again).

### Users panel

**Settings → Users** (admins only) has two tables:
- **Users**: email, name, role (change it in place; a `saml` account's comes from the identity
  provider, see [SAML](#saml)), origin (`local` or `saml`; **SSO** means no password), active,
  **Never turn off** (a break-glass account, which [inactivity](#inactive-accounts) never turns
  off), state, last sign-in, and the password, sign-out and delete buttons.
- **API tokens**: name, the account it acts as, who made it, when, when it expires, and its state
  (active, expired, revoked).

## Sign-in

OpenTrack accepts several ways of signing in at once. For each request it tries, in order: a client
certificate, an API token, its own session cookie, then (when trusted) OpenStare's session or
token. The **Single sign-on** panel in Settings → Security holds the sign-in settings, in one form
with one **Save**: the **Password sign-in** switch, then a section each for SAML, OpenStare sign-in
and client certificates. Changes take effect at once and go in the decision log.

### Password sign-in

Local accounts sign in with email and password at `/login`. The session is a signed `ot_session`
cookie (HttpOnly, SameSite=Strict, and Secure over TLS). It lasts at most 24 hours; see
[Session limits](#session-limits).

**Password sign-in**, at the top of the Single sign-on panel: off means accounts sign in only with
SAML or OpenStare. You can turn it off only when one of those is on, and not from a session that
signed in with a password, so a working way back is proven first: turn it off while signed in by
SAML (keeping SAML on in the same save), through OpenStare, or with an admin's API token
(`PUT /api/v1/auth/settings`).

### SAML

OpenTrack is the SAML service provider; your identity provider (Keycloak, Entra ID, ADFS…) signs
users in. OpenTrack sends an unsigned request with the HTTP-Redirect binding, and takes the
signed response with the HTTP-POST binding. Sign-on started at the identity provider is not
supported.

1. **Set `OT_PUBLIC_URL`** to the address browsers use, such as `https://opentrack.example.org`,
   and restart. The **Service provider** row in the Single sign-on panel's SAML section then shows the two addresses
   the identity provider needs:
   - entity ID and metadata: `<OT_PUBLIC_URL>/api/v1/auth/saml/metadata`
   - assertion consumer service (ACS): `<OT_PUBLIC_URL>/api/v1/auth/saml/acs`
2. **Register OpenTrack at the identity provider** with those two addresses (or give it the
   metadata URL). It must:
   - put the user's **email address in the NameID** (OpenTrack finds or makes the account by it);
   - **sign** its responses or assertions;
   - accept **unsigned** authentication requests (in Keycloak, turn **Client signature required**
     off);
   - send the user's role in an attribute (in Keycloak, a role list mapper; in Entra ID, app roles,
     sent as `http://schemas.microsoft.com/ws/2008/06/identity/claims/role`).
3. **Paste the identity provider's metadata XML** into **IdP metadata** and choose **Read
   metadata**. The IdP entity ID, sign-in URL and signing certificate below it then show what the
   metadata says. They are read-only: the metadata alone is what OpenTrack checks sign-ons against,
   and the three are read from it again at every save. To change them (after a certificate
   rollover, say), paste the new metadata. Metadata that cannot be read is refused while SAML is
   on.
4. **Role attribute:** the attribute's name (or friendly name), exactly as the provider sends it.
   Default `Role`.
5. **Role mapping:** rows of attribute value → OpenTrack role. Case does not matter. The rows are
   tried in order and **the first match wins**, so put the most specific values first. A user with
   several values gets the role of the first row any of them matches.
6. **Default role:** the role of a user whose values match no row. **None** refuses them.
7. **Allow admin:** off (the default), single sign-on gives at most `track_manager`, whatever the
   mapping says. Turn it on only if the identity provider should make admins.
8. **Button label:** the text of the single sign-on button on the sign-in page. Default "Sign in
   with SSO".
9. Turn **Enabled** on and **Save**. Test it from a private browser window before signing out.

What happens at a sign-on:
- The first time, OpenTrack makes an account (origin `saml`, no password) with the mapped role.
- Next time, a `saml` account takes the role the provider gives it now. If its values map to no
  role and there is no default, it is refused. The identity provider is authoritative for the
  role: Settings → Users shows a `saml` account's role read-only, and the API refuses to change
  it (409). To change it, change the mapping here or the user's role at the provider.
  (`opentrack user role` on the server still can, as a break-glass, until the next sign-on.)
- If the account with that email was made another way (a local account), the sign-on is refused:
  an identity provider never takes over a local account or its role. Use a different email for
  the local account, or delete it so SAML makes a new one.
- A turned-off account is refused.
- Each assertion can be used once, and must answer a request OpenTrack made in the last 5 minutes.

A refused sign-on returns to the sign-in page with "Single sign-on failed". The reason is in the
log (`SAML sign-on refused`, with a `reason`).

The build must include SAML: the Docker image does. A lean build (`--no-default-features`) shows
"Not in this build" instead.

### OpenStare sign-in

When OpenTrack runs beside OpenStare on the same host, users can sign in once. OpenTrack then
accepts:
- a browser signed in to OpenStare on the same host name (its `session` cookie), and
- an OpenStare API token, sent as `Authorization: Bearer`.

For each, OpenTrack asks OpenStare's `/api/auth/me` who it is, and keeps the answer for 30
seconds. So an OpenStare sign-out or revocation holds here within 30 seconds. No OpenTrack
account is made; the decision log records the user's OpenStare email.

In the Single sign-on panel's **OpenStare sign-in** section:
- **API URL:** OpenStare's API as the OpenTrack server reaches it. Default `http://127.0.0.1:3001`.
- **Sign-in page URL:** OpenStare's sign-in page as browsers reach it. It puts a "Sign in with
  OpenStare" button on OpenTrack's sign-in page. Empty: no button.
- **Role mapping:** OpenStare roles (`admin`, `operator`, `analyst`, `guest`) → OpenTrack roles.
  Default: admin → admin, operator → track_manager, analyst → viewer. A role not listed (such as
  `guest`) is refused.

### Client certificates

With mutual TLS, a machine can sign in with its certificate alone.

1. Serve over TLS with a client CA: `OT_TLS_CERT`, `OT_TLS_KEY` and `OT_TLS_CLIENT_CA`.
2. Make an account for the machine (with no password), with the role it needs.
3. In the Single sign-on panel's **Client certificates** section, add a row: the certificate subject's common name (CN), and the
   account's email. Save.

A certificate the CA signed whose CN is listed signs in as that account, while the account is
active. Other callers can still use passwords and tokens.

**Revocation.** Every client certificate presented is checked for revocation right after the TLS
handshake, before any request is served:

1. **OCSP.** The server asks the certificate's OCSP responder: the URL in its authority
   information access extension, or `OT_TLS_CLIENT_OCSP_URL` for every certificate (an internal
   responder, or one reached through a firewall rule; see
   [Ports, protocols and services](#ports-protocols-and-services)). The answer counts only if it
   is signed by the issuing CA, or by a responder certificate that CA issued directly with the
   OCSP Signing extended key usage (valid now); names this certificate; carries the request's
   nonce, if it carries one; and is current (thisUpdate not in the future and nextUpdate not
   past, allowing 5 minutes of clock skew; an answer without a nextUpdate must be under an hour
   old). **Good** lets the certificate in; **revoked** refuses it. Both are remembered until the
   answer's nextUpdate, at most an hour (5 minutes without one), for up to 10,000 certificates,
   and a browser's parallel connections share one query.
2. **Revocation lists.** With no usable OCSP answer, `OT_TLS_CLIENT_CRL` decides: the certificate
   names no responder, the responder can't be reached within 5 seconds or answers with an HTTP
   error (it is then left alone for 30 seconds), or its answer is unusable (unsigned, for another
   certificate, stale, an error status) or **unknown**. Unknown isn't a revocation: it means the
   responder doesn't vouch for the certificate, so the lists must. Keep them current (a list past
   its next update refuses every certificate its CA issued), for example with a daily job that
   downloads your CA's CRLs into the directory.
3. **Neither.** With no OCSP answer and no revocation list, the certificate is refused (fail
   closed).

A refused certificate's connection is closed and logged as `client certificate refused`; the audit
record gets `certificate_refused` (subject, serial, reason, `checked`: `ocsp`, `crl` or `none`, and
the responder). An OCSP query that gave no usable answer is logged and audited as
`ocsp_unavailable` (outcome, responder, and what the lists then decided).

> **Upgrading to 0.4.5.** Client certificates are now checked after the handshake, OCSP first.
> A site with `OT_TLS_CLIENT_CA` but no `OT_TLS_CLIENT_CRL`, whose certificates name no OCSP
> responder, has its certificates **refused** from this release: set `OT_TLS_CLIENT_OCSP_URL`,
> or give the lists with `OT_TLS_CLIENT_CRL`. Let the server reach the responder (outbound HTTP).

### Sign-in turned off

`OT_AUTH=off` turns sign-in off: every caller, from anywhere, is an admin, and the decision log
records them as `anonymous`. The server logs a warning at start. Use it only on a development
machine that nothing else can reach. Never in production.

## Banners

### Classification banner

**Settings → Banners.** A fixed bar at the top and bottom of every page, including the sign-in
page:
- **Classification banner:** on or off.
- **Classification text:** up to 128 characters, such as `UNCLASSIFIED // FOR OFFICIAL USE ONLY`.
- **Color preset:** OpenStare's colours for Unclassified, CUI, Confidential, Secret and Top Secret;
  or set **Background color** and **Text color** (`#rrggbb`).

Each OpenTrack sets its own banner, whatever the OpenStare it feeds shows. Browsers pick up a change
within a minute. The banner marks the system; it does not mark the tracks (see
[Security labels](#security-labels)). Its text also marks exported files with no labelled content,
whether or not the banner is shown, so keep it right (see [Markings](#markings)).

### Notice and consent

The **Warning banner** is a notice users must accept after signing in, such as consent to
monitoring, as OpenStare's warning banner:
- **AGREE** goes on; **DECLINE** signs the user out.
- It is asked at every sign-in. The server keeps the acceptance and refuses the API (except the
  sign-in calls) until it has it, so no screen or script gets round it; accepting is audited
  (`consent_accepted`). Client-certificate and OpenStare access, which have no sign-in, are asked
  once a day. API tokens are programs, and are not asked.
- Admins accept it too; to change a wrong text, accept it and edit it here.

Turn it on and give the text (up to 20,000 characters). Save.

## Security labels

A source can label everything it reports with a security label, in the fields OpenStare's ICD
reserves: `classification`, `restrictions` and `sharing`. Set it in the source's pipeline, publish
stage, **Security label** (Sources → a source → Pipeline):
- **Classification:** such as `SECRET`.
- **Restrictions:** comma separated, such as `NOFORN, ORCON`.
- **Sharing:** who it may be released to, such as `REL TO USA, FVEY`.

The label goes into each published track message as `security`
([docs/nats-output.md](../nats-output.md)). A track that several labelled sources report for
carries:
- the **highest classification** among them;
- **every restriction** any of them has;
- only the **releasability they all share** (`NONE` when they share none).

Which classification is highest comes from the list in **Settings → Security → Security labels**.
It runs lowest first, and by default reads `UNCLASSIFIED`, `CUI`, `CONFIDENTIAL`, `SECRET`.
- **Reorder:** drag a row by its handle.
- **Add or remove:** **Add classification** adds a row; the × removes one.
- **Save** when you're done.

Case does not matter, `U`, `C`, `S` and `TS` stand for their names, and caveats after `//` are
ignored when ranking. A classification that isn't in the list ranks above all of them, so a track
is never marked too low. Add `TOP SECRET` (or your own levels) if your sources carry them.

OpenTrack does not hide or withhold anything by label or by the user's clearance: everyone signed
in sees every track, and every output sends every published track. Labels are marked, not enforced;
enforcement is downstream.

### Markings

Every output shows a labelled track's label as one **portion marking**, in common US marking
syntax:
- the classification abbreviated: `U`, `C`, `S`, `TS` (others, such as `CUI` or `NATO SECRET`, in
  capitals as written);
- after `//`, the restrictions in their order, in capitals, each once, separated by `/`
  (with any `//` caveats written in the classification first);
- last, the releasability as `REL TO` with `USA` first, then other country codes, then
  organisations (`FVEY`, `NATO`), each alphabetical. Releasable to nothing in common (`NONE`)
  is `NOFORN`, and a `NOFORN` restriction drops the `REL TO` list.

So `SECRET` with `ORCON` and sharing `GBR, USA` is `(S//ORCON/REL TO USA, GBR)`, and
`UNCLASSIFIED` with `FOUO` is `(U//FOUO)`. A track without a label has no marking.

Where the marking appears:
- **TAK**: a `__security` element and the start of the remarks (see
  [What TAK receives](#what-tak-receives)).
- **NATS**: the label itself, as `security` ([docs/nats-output.md](../nats-output.md)).
- **The UI**: the track card (**Marking**) and the track table's **Marking** column; the track
  API (`GET /api/v1/tracks/<UID>`, `GET /api/v1/tracks`) returns it as `marking`.
- **Exported files**, each with an overall marking:

| File | Overall marking | Per item |
|---|---|---|
| Tracks, GeoJSON (Settings → Data) | top-level `security.marking` | each labelled feature's `security` (the label) and `marking` |
| Tracks, CSV | the first line, above the header | the `classification` column (empty when unlabelled) |
| Audit record CSV | the first line | none |
| Registry XLSX / CSV | the first row, above the header (and each printed page's header and footer in XLSX) | none |
| Configuration export | the `marking` member | the sources' labels, in their specifications |

A file's overall marking is the banner's text (**Settings → Banners**, whether or not the banner
is shown) when nothing in it has a label. Otherwise it is the marking of its items' labels combined
as a fused track's are (highest classification, every restriction, shared releasability), with the
banner's text standing for any item without a label, so a file is never marked below the system
it came from. For the configuration export the items are the sources. A registry sheet imports
with or without its marking row.

## Settings tab

The **Settings** tab, panel by panel, in order. Viewers and track managers see General, Data and
Banners, but can't change them.

| Panel | What it holds | See |
|---|---|---|
| **General** | **Site name** (shown in the header and the browser title; up to 64 characters). The **site code** beside it is `OT_SITE_CODE`, read only. **Position history:** how long each track's positions are kept (hours, 0 to 720; empty 12) and at most one point per track how often (seconds, 0 to 3600; empty 10). Memory is about 130 bytes a point: 2,000 tracks for 12 h every 10 s take about 1.1 GB of Redis. **Basemap tiles:** an XYZ tile URL for the maps' background (empty: the built-in country outlines; others see only whether it is on). **Plugins:** codec, tracker and scorer plugins. | [Basemap tiles](#basemap-tiles), [Plugins](#plugins) |
| **Users**, **API tokens** | Accounts, with the break-glass ones marked **Never turn off**, and machine tokens (admins only). | [Users](#users) |
| **Nodes** | Sharing the picture with other OpenTrack nodes (admins only). | [Multi-node](#multi-node) |
| **TAK output** | Cursor-on-Target outputs to TAK: a TAK Server, multicast, or TAK clients connecting to this node (admins only). | [TAK output](#tak-output) |
| **Data** | Export: live tracks as GeoJSON or CSV, the registry as XLSX, and the full configuration (admins only; it holds secrets), with its import while the node is empty. **Purge:** retire every live track. | [Configuration export](#configuration-export), [Purge](#purge) |
| **Banners** | Classification banner and warning banner. | [Banners](#banners) |
| **Security labels** | The classification order a fused track's label is chosen by (admins only). | [Security labels](#security-labels) |
| **Single sign-on** | The sign-in settings, with one Save: password sign-in on or off, SAML, OpenStare sign-in and client certificates (admins only). | [Sign-in](#sign-in) |

The General, Nodes, TAK output and Banners panels share one draft: **Save** on any of them saves
them all.

### Basemap tiles

The maps (Track Management and the source preview) draw the built-in country outlines, which need
nothing from outside. For a full map instead, give **Settings → General → Basemap tiles** the URL
of an XYZ raster tile server, such as `https://tiles.example/{z}/{x}/{y}.png`:
- An absolute `http` or `https` URL of up to 2048 characters with `{z}`, `{x}` and `{y}` in it.
  No other placeholders: `{s}` (a choice of subdomains) is refused, so give one host. No
  `user:pass@`; a key the tile server takes can go in the query, e.g. `?key=...`.
- The tiles are PNG, JPEG, WebP or AVIF, up to 4 MiB each. Empty turns tiles off.

**OpenTrack fetches the tiles for the browser** (`GET /api/v1/basemap/{z}/{x}/{y}`, any signed-in
account), so:
- browsers need no route to the tile server, and an internal one works;
- the page's content security policy is unchanged (this origin only);
- the URL, with any key in it, never reaches browsers. Only admins see it in the settings; everyone
  else sees whether tiles are on, and the decision log records only that it was set.

A tile the server doesn't have (open ocean, say) is left empty; a tile server that fails, is slow
(10 s), redirects or sends anything but an image gives that tile a 502, logged with the server's
host and status.

**Tiles are not cached on the OpenTrack server.** Each browser keeps a tile for a day, so a room
of operators still makes one request per tile per browser.

The tile provider's terms are yours to meet, including any attribution it requires (OpenTrack
shows none). OpenStreetMap's public tile servers forbid heavy use such as this; use a provider that
allows it, or a tile server of your own.

## Sources and correlation

Configuring what OpenTrack ingests and how it correlates is an admin's job. Operators see the
result on the Sources and Correlation tabs.

### Sources

**Sources → Add source** runs a wizard: transport, framing, codec, then the pipeline designer,
with a live preview of each stage on sample data. Enable a source to start it. Each save is a
revision: the source's **History** tab lists them, with who saved each and the spec it saved. Write secrets as
`${env:NAME}`, resolved when the source starts. Only admins see a source's secrets: for viewers and
track managers, passwords, tokens, header and metadata values, credentials in URLs and API keys in
messages show as `••••••` (`${env:…}` references stay visible). See the README, "Sources and pipelines", and `docs/examples/`.

**Probing a feed.** The wizard's probe connects with the transport as set and captures what
arrives. Choosing Cursor-on-Target XML on a TCP transport frames by the end tag `</event>` (CoT
events arrive back to back, often over several lines). A probe that connects but captures nothing
says why it may be: **no bytes arrived** (the feed is idle; the port needs TLS, since a TLS server
waits silently for a handshake; or the port only takes data in, as a TAK Server input does), or
**bytes arrived but never made a frame** (check the framing). To read a TAK Server, connect to its
streaming port (8089) with TLS and a client certificate it issued; its input ports never send.

**A listening source must authenticate its senders.** `tcp_server` needs TLS with a client CA
(mutual TLS, a certificate required: not `client_cert_optional`); `grpc_server` needs that or a
bearer `token`. Otherwise the source is saved and run only with the risk accepted on it,
`"unauthenticated": "accepted"` (the red **Accept unauthenticated senders** toggle on its Transport
tab; always needed for `udp`, which cannot carry TLS). The flag is refused where it is not needed
(a client transport, or a listener that authenticates), so it only ever marks a real exception;
each one needs the authorising official's acceptance ([hardening checklist](../security/hardening.md#encrypt-every-link)).
The source list shows an UNAUTHENTICATED badge on such sources, and the decision log records the
flag set or cleared. **Upgrading to 0.4.5:** an enabled listener saved before without sender
authentication or the flag is not started; its state is *not started* and its Status tab says why.
Add a client CA or token, or set the flag, and save. A configuration import is checked the same way.

**UDP multicast.** A `udp` source joins `multicast_group` if one is set, IPv4 (`239.2.3.1`, with
`bind` `0.0.0.0:port`) or IPv6 (`ff15::6969`, with `bind` `[::]:port`); the group and the bind must
be the same IP version. `multicast_interface` picks the interface to join on: an IPv4 address of
this node for an IPv4 group, an interface name (`eth0`) or index (`2`) for an IPv6 group. Blank:
the system's choice. A name is looked up when the source starts, so a missing interface shows as
the source's error.

The pipeline designer's **Live preview** (right-hand pane) is a dry run of the pipeline as edited,
not yet saved, over the source's stored samples; **Capture samples** replaces them with up to 20
frames (or 15 s) of the live feed. Nothing is published. The pane shows the first 5 samples, each
a decoded record in decode order (the first frame's, then the next frame's if it has fewer), under
its own header (#1, #2…) as pretty-printed JSON, **as they are after the stage selected on the
left**:
- **Transport:** each frame the samples came from, once, as received ("frame 1 (samples
  #1–#5)"): JSON pretty-printed, other text as it is, binary escaped (`\xNN`) with its length; a
  frame over 64 KiB shows only its start.
- **Decode:** the record. A frame that does not decode is a sample of its own, with the error.
- **Reject:** the record, if kept. **Map:** what the mapping made of it: the observation (before
  the identity join fills it in, when there is one), or a static rule's identity fields.
- **Identity join, Entity links, Affiliation, Filter, Tracker, Throttle:** the observation after
  each. A tracker's report shows under the sample whose plot made it.
- **Publish:** the message as it would be published for a new system track seeded from the
  observation (the GOLD fields and the output schema's attributes; its UID is correlation's to
  assign, and correlation decides whether and when it is published).

Sample #1 is the same record at every stage. A sample dropped on the way says where and why
("rejected: no_position", "dropped by Filter: drop_if matched"), and a tracker waiting for the rest
of a scan says it held the sample. The frames' other records are processed too (they count, and
move the tracker and throttle) but not shown. Above the samples, one line counts what the pipeline
did with all the stored samples (frames, records, dropped…), with any errors.

**Filter** keeps or drops observations by condition, in two parts: **Keep only if** (an
observation that does not match is dropped) and **Drop if** (one that matches is dropped, even
when it passes Keep only if). Each is built from rows of **field**, **test** and **value**,
matched **all** or **any**:
- **Field:** the fields the stored samples hold at this stage, each with an example
  (`source_track_key (e.g. tms-OTK000000123)`); **Other path…** takes any path, such as one only
  some records carry.
- **Test:** is, is not, is one of, is none of (values separated by commas), starts with, does not
  start with, contains, does not contain, matches pattern, does not match pattern (a regular
  expression), is more than, is at least, is less than, is at most, is present, is missing.
- **Value:** saved as a number or true/false when the samples hold one at that field.

Under the rows a line says the condition in words, and under both, what the filter as edited does
to the stored samples ("drops 55 (e.g. tms-OTK000058833, …)"). **Edit as JSON** shows the
condition as the pipeline stores it; a condition the rows cannot show (groups inside groups, a
transformed value) opens there. For example, to stop a TAK Server source reading back the tracks
OpenTrack itself sends to that server: **Drop if**, `source_track_key`, starts with, `tms-`.

### Output schema

The **Schema** tab defines the attributes every published track carries. Edit a draft, then
**Publish** it: a published version never changes, and every live track whose attributes change is
republished. A source whose mapping targets an unpublished version doesn't start.

### Correlation settings

The **Correlation** tab's **Settings** panel (admins save; others see it) sets how tracks pair:
identifiers only, kinematics, or kinematics vetoed by metadata; auto or suggest mode; the
kinematic test; splits; and the **output filter**, which holds tracks back by area, affiliation,
domain, track type or confidence. Saved settings apply while the engine runs and override
`OT_CORRELATION`. **Defaults** restores the built-in values. See
[docs/algorithms.md](../algorithms.md).

### Tracker profiles

A source's tracker stage can load a **tracker profile**, a sensor's tracker settings as a JSON file.
Profiles shipped with OpenTrack are read from `OT_PROFILES_DIR`; profiles imported or saved in the
pipeline designer go to `profiles/trackers/` beside the database. Back that directory up with the
database.

## Plugins

Codecs, trackers and pairing scorers of your own, beside the built-in ones. **Settings → General → Plugins**
(admins change it; others see it):
- **Add plugin:** a WebAssembly component (`.wasm`, run sandboxed inside OpenTrack), or an
  external plugin's address (a program of its own serving the plugin interface on a socket) and
  its **secret**. **Replace** installs a new build of a plugin with the same name, keeping its grants and whether
  it is enabled.
- **Enable** toggle, **Check** (loads it and opens each kind it provides), **Secret** (external
  plugins), **Grants**, **Delete**.
- **Secret** (external plugins): OpenTrack and the plugin prove to each other that they hold it
  (HMAC-SHA256 challenge and response each way) on every connection; a plugin that fails is
  refused, logged as an error and audited (`plugin_auth_failed`), and its status says why.
  **Generate** makes one with the FIPS DRBG and shows it once: start the plugin with it
  (`OT_PLUGIN_SECRET` or `--secret-file`), then add or save. You can paste your own (32 characters
  or more) or `${env:NAME}`. The API never returns it, only whether one is set. Only a plugin on a
  unix socket under the data directory may have none: filesystem permissions decide who can
  listen there. Details: [docs/plugins.md](../plugins.md#authentication).
- **Grants** (WebAssembly only): memory ceiling (MB), time per call (ms; a call that runs longer is
  stopped), network addresses it may connect to, server directories it may see (read only unless
  `rw`), and environment variables.
- **Used by** shows the sources and correlation settings that use it.

A WebAssembly plugin is stored in the database, so it is backed up with it. An external plugin is
only an address and its secret: run and back up its program yourself.

**Upgrading from 0.4.4 or earlier:** external plugins had no secret, and after the upgrade one
without a secret does not load (its status says *no secret*) unless it is on a unix socket under
the data directory. For each: update it to the current SDK, **Generate** a secret in its
**Secret** dialog, restart the plugin with that secret, then **Save**.

Plugins are loaded without a restart. From the command line: [Plugin commands](#plugin-commands).
Writing plugins: [docs/plugins.md](../plugins.md).

## Multi-node

Several OpenTrack nodes (server sites, or a drone swarm) can share one track picture with no node
in charge: one track number per object on every node, the node that sees a track best reports it,
and a track manager's decision on any node holds on all of them. OpenTrack does not do the
networking: the `link` role puts sync messages on this node's NATS (`ot.sync.out.*`,
`ot.sync.in.*`), and a networking package, or `opentrack bridge` between server sites, carries
them. Design: [docs/multi-node.md](../multi-node.md). Message format:
[docs/sync-icd.md](../sync-icd.md). Two-site example: `docs/examples/two-sites/`.

To turn it on:
1. Give every node its own `OT_SITE_CODE`, and run the `link` role (`all` includes it).
2. Carry `<OT_SYNC_PREFIX>.out.*` to the other nodes' `<OT_SYNC_PREFIX>.in.*`.
3. In **Settings → Nodes**:
   - **Share the picture:** on.
   - **Trusted nodes:** the other nodes' site codes, comma separated. Messages from any other node
     are dropped.
   - **This node's key:** copy this node's public key and give it to each other node's admin.
     Read the fingerprint out to them over another channel (a phone call, the deployment plan).
   - In the table, **Pin key** for each trusted node: paste that node's public key and compare the
     fingerprint with the one its admin gives you. A trusted node with no key shows **no key:
     refused**: its messages are refused until you pin one.
   - **Sending budget:** this node's share of the link in kbit/s (0: no cap). Past it, new tracks,
     state changes and the largest drifts go first.
   - **Receive only:** apply other nodes' track management here but accept none from this node's
     users (a node with no operator, such as a drone).
   - **Share the profile:** publishing an output schema or saving correlation settings on any node
     applies on every node that shares the profile; the later change wins.
4. Save. The table shows each trusted node: heard, silent (not heard for 30 s) or never heard, how
   many of its decisions this node holds, and its key's fingerprint. Below it: how many tracks this
   node reports and holds only from others, and the decisions applied, waiting for their tracks,
   superseded or failed; and, if any, the messages refused.

#### Signed sync messages

Every sync message is signed with the sending node's Ed25519 key (FIPS 186-5, in the FIPS module) and
accepted only if it verifies with the key pinned for its site code. Pinning, replacing and removing a
key are an admin's, and each is recorded in the decision log and the audit record
(`sync_key_pin`, `sync_key_remove`, with the key before and after).

A node refuses, counts and logs (at most once a minute per node) a message:
- from a node not in **Trusted nodes**, or with no key pinned;
- whose signature does not verify with the pinned key. This is also audited
  (`sync_signature_refused`), at most once a minute per node: someone may be using that node's
  name. It is also what you see after a node's key changed and the new one is not pinned yet;
- whose clock is more than 5 minutes from this node's (keep nodes on NTP or GPS time);
- already accepted once (a replay).

The key is made on first start and kept in `sync.key` beside the database. Back it up with the
database. If you lose it, the node makes a new one: pin the new key on every other node. To replace
a node's key on purpose (it may have been exposed), stop the node, delete `sync.key`, start it, and
pin the new key everywhere; remove the old one from nodes that no longer trust it.

**Upgrading to 0.4.5.** Sync messages are now version 2 (signed). Nodes on 0.4.4 or earlier don't
understand them, and 0.4.5 refuses theirs, so upgrade every node of a swarm together. After the
upgrade no trusted node has a key pinned, so every message is refused until you exchange keys: on
each node, copy **This node's key** and pin it on every other node. The **Nodes** panel shows
**no key: refused** against each node still missing one.

The registry, sources and plugins are not shared between nodes.

### The bridge's NATS credentials and TLS

`opentrack bridge` signs in to each node's NATS the way OpenTrack's own NATS connection does, and
with TLS through the same FIPS module. What you give with the `--nats-*` flags (or the
`OT_BRIDGE_NATS_*` variables) applies to every `--node`:

| Flag | Variable | |
|---|---|---|
| `--nats-ca` | `OT_BRIDGE_NATS_CA` | Trust this CA (PEM), and refuse a connection without TLS. |
| `--nats-cert`, `--nats-key` | `OT_BRIDGE_NATS_CERT`, `OT_BRIDGE_NATS_KEY` | Mutual TLS: this client certificate and key (PEM). |
| `--nats-creds` | `OT_BRIDGE_NATS_CREDS` | A credentials file (JWT and NKey); the nonce is signed in the FIPS module. |
| `--nats-user` | `OT_BRIDGE_NATS_USER` | A user, with a password. |
| `--nats-password-file` | `OT_BRIDGE_NATS_PASSWORD_FILE` | A file holding the password. |
| `--nats-token-file` | `OT_BRIDGE_NATS_TOKEN_FILE` | A file holding a token. |
| `--nats-password`, `--nats-token` | `OT_BRIDGE_NATS_PASSWORD`, `OT_BRIDGE_NATS_TOKEN` | The password or token itself. Prefer the files or the variables: a flag shows in the process list. |

A node whose NATS needs something else says so after its URL, with `;name=value` options: `ca`,
`cert`, `key`, `creds`, `user`, `password-file` and `token-file`. An empty value drops the shared
one. Passwords and tokens go in files there, never on the command line:

```sh
OT_BRIDGE_NATS_CA=/run/secrets/ca.pem \
OT_BRIDGE_NATS_CERT=/run/secrets/bridge.pem OT_BRIDGE_NATS_KEY=/run/secrets/bridge.key \
opentrack bridge \
  --node tls://nats-a.site-a:4222 \
  --node 'tls://nats-b.site-b:4222;ca=/run/secrets/site-b-ca.pem;creds=/run/secrets/site-b.creds'
```

The bridge refuses to start on half a key pair, a user without a password (or the other way
round), more than one way of signing in to one node, a password or token given as a node option,
or a file it cannot read. It logs each node's prefix, whether it uses TLS and how it signs in
(`creds`, `token`, `user` or `none`), never a secret or the user information in a URL. A node
whose server refuses the certificate or the credentials is retried; the server's log says why.

To check a node's link by hand: start the bridge with `OT_LOG=info`, then list the server's
connections (`curl http://<nats>:8222/connz?subs=1`, with its monitoring port on). The
`opentrack-bridge` connection should show a `tls_version` and a subscription to
`<prefix>.out.>`. Stop the bridge, drop `--nats-cert` and start it again: the server should log a
TLS handshake error and show no `opentrack-bridge` connection.

<a id="tak-output"></a>
## TAK output

OpenTrack streams its published tracks to TAK (ATAK, WinTAK, iTAK, TAK Server) as
Cursor-on-Target (CoT) XML, beside NATS. The `cot` role sends them (`opentrack all` runs it); it
reads the same outbox as the NATS writer in a consumer group of its own (`track-cot`), so a slow or
broken TAK link never holds up NATS, and it sends exactly the tracks NATS gets: those the publish
rule and the output filter let out. A track that is withdrawn, merged away, deleted or dropped is
deleted in TAK too.

Only tracks and their deletes are sent: no bearings, areas, groups' drawings or history, and no
security label handling (events go out as the tracks are; mark the network, not the events). TAK
Protocol (protobuf) is not supported: TAK Server and every TAK client accept CoT XML.

### Outputs

**Settings → TAK output** (admins) holds any number of outputs. **Add output**, fill it in,
**Apply**, then **Save** (the panel shares the page's draft). The role picks up a saved change
within 5 seconds: a new or changed output starts (or restarts), one turned off or deleted stops.
There is no restart.

Every output has:

| Field | |
|---|---|
| **Name** | 1 to 32 letters, digits, `-` or `_`, unique. It names the output in logs, the status and the metrics. |
| **On** | Send to it, or keep it without sending. |
| **Stale after** | Seconds after a track's **last report** that TAK lets it go (10 to 86400, default 60). OpenTrack re-sends a track every half of this until then, then stops: a track that stops reporting greys out and leaves TAK this long after its last report, even while OpenTrack still holds it (OpenTrack keeps a silent track for up to 6 hours). A new report brings it back. If OpenTrack itself stops sending, tracks leave TAK the same way. |
| **Remarks** | Put the OpenTrack track number and the sources reporting the track in the event's remarks. A labelled track's remarks carry its marking whether or not this is on (see [What TAK receives](#what-tak-receives)). |

and one of three deliveries:

- **TAK Server**: OpenTrack connects to a TAK Server's streaming input, as a TAK client does, and
  TAK Server shares the tracks with its users. **Host**, **Port** (8089 with TLS, the usual; 8087
  plain TCP) and **TLS**: the **CA file** that signed the server's certificate (empty: the system
  roots), the **client certificate** and **client key** TAK Server's 8089 input requires (PEM,
  converted from TAK's `.p12`: see [TAK certificates](#tak-certificates)), and an optional **server name** to check the certificate against. OpenTrack reconnects after a
  failure, waiting 1 s and doubling up to a minute, and sends the whole picture on every connect.
- **Multicast**: UDP datagrams, one event each, to a **group** and **port** (TAK's SA multicast,
  `239.2.3.1:6969`, by default), with a **TTL** (1: this network only; the hop limit for IPv6) and
  an optional **interface** to send from: an IPv4 address of this node for an IPv4 group, an
  interface name (`eth0`) or index for an IPv6 group (such as `ff15::6969`). ATAK and WinTAK on the
  network hear it with no setup. A unicast address also works (one receiver, such as a TAK Server's UDP input). The picture is sent
  when the output starts; after that each track at least every half of its stale time, while it is still reporting.
- **Listen for clients**: OpenTrack is the server. It listens on **Listen on** (such as
  `0.0.0.0:8089`) and ATAK or WinTAK connect to it as to a TAK Server (a server connection to this
  node and port; SSL when TLS is on). Each client gets the whole picture, then every change. With
  **TLS**: the server **certificate** and **key** (PEM; clients must trust its CA), and optionally a
  **client CA** (clients must present a certificate it signed: mutual TLS, as TAK Server's 8089
  does), **certificate optional**, and **revocation lists** (PEM or DER files or directories;
  reloaded within a minute of a change). Many clients can connect. Each has a queue of 8,192
  events; a client that falls that far behind is dropped (logged, counted as an error and under
  `dropped`) so it never holds up the others; it gets the picture again when it reconnects.

Certificate, key and CA fields are paths on the node running the `cot` role (`${env:NAME}`
references work, as in sources). Keys must be unencrypted PEM; keep them readable only by
OpenTrack. All TLS runs through the node's FIPS 140-3 module, as every other link does.

### TAK certificates

OpenTrack reads certificates and keys as **PEM** only. TAK Server's certificate scripts
(`makeRootCa.sh`, `makeCert.sh`) and ATAK use **PKCS#12** (`.p12`) bundles, so convert them with
OpenSSL. OpenTrack doesn't read `.p12` files itself: the ciphers that protect most `.p12` bundles
(RC2, 3DES, SHA-1 key derivation) are outside the FIPS module. TAK's scripts protect their
bundles with the password `atakatak` unless you changed it.

**Pushing to a TAK Server (8089).** Make a client certificate for OpenTrack on the TAK Server
(`./makeCert.sh client opentrack`, in TAK Server's `certs` directory). If TAK Server checks
users, authorise that certificate there. Then, on the TAK Server:

```sh
cd /opt/tak/certs/files
openssl pkcs12 -in opentrack.p12 -clcerts -nokeys -out opentrack.pem          # client certificate
openssl pkcs12 -in opentrack.p12 -nocerts -nodes -out opentrack.key           # its key, unencrypted
openssl pkcs12 -in truststore-root.p12 -nokeys -out tak-ca.pem                # TAK Server's CA
```

Copy the three files to the node running the `cot` role, readable only by OpenTrack
(`chmod 600 opentrack.key`). Set them as the output's **Client certificate**, **Client key** and
**CA file**. OpenSSL 3 may refuse old bundles with "unsupported algorithm"; add `-legacy` to
those commands.

**ATAK and WinTAK connecting to OpenTrack (Listen for clients, with TLS).** Clients need
OpenTrack's CA as their trust store. With a client CA set, they also need their own certificate
from that CA. Both go to the devices as `.p12`:

```sh
# The trust store: OpenTrack's server CA, for the device's "CA certificate" / truststore.
openssl pkcs12 -export -nokeys -in opentrack-ca.pem -out opentrack-truststore.p12 -passout pass:atakatak
# A device's own client certificate and key (signed by the output's client CA).
openssl pkcs12 -export -in device.pem -inkey device.key -certfile client-ca.pem \
  -out device.p12 -passout pass:atakatak
```

A TAK Server CA works as the client CA: devices already enrolled with that TAK Server can then
connect to OpenTrack with the certificates they have. Use `tak-ca.pem` from above as the
output's **Client CA**.

> **Plain TCP and multicast send the picture in the clear.** Anyone on the network path can read
> every track, and anyone who can reach a plain listening output can connect. Use TLS (with client
> certificates for a listening output) wherever the network is not itself protected. Multicast is
> always plaintext. The status table marks each output TLS or plaintext, and the role logs a
> warning when a plaintext output starts.

### What TAK receives

Each published track is one CoT event:

| CoT | From |
|---|---|
| `uid` | `tms-<UID>`, the track's id on NATS too. |
| `type` | The track's symbol: a 2525C SIDC as `a-<affiliation>-<dimension>-<function…>` (`SFSPCLDD---` is `a-f-S-C-L-D-D`; exercise identities as their real ones); a 2525D SIDC as its identity and symbol set (`a-h-G-U` for a hostile land unit, `a-f-G-E`, `a-n-G-I`, `a-f-S`; the entity code is not translated); a CoT type as it is. A track with both a CoT type and a SIDC (an entity stores both) sends the more specific of the two (the CoT type on a tie). Without one (or for a tactical graphic or weather symbol), affiliation and domain: `a-h-A`, `a-f-S`, `a-u-G` (ground when the domain is unknown). An explicit affiliation (an entity, a track manager) overrides the symbol's. |
| `how` | `m-f` (machine, fused). |
| `time`, `start` | The track's last report (its observation time; never later than now, for a source whose clock runs ahead). Re-sending a track does not make it look newer. |
| `stale` | `time` plus the output's stale time. A track past it is not sent (not re-sent, not in the picture a client gets on connecting) until it reports again. |
| `point` | Latitude, longitude, `hae` (height above the ellipsoid) and the error: `ce` the circular 1-sigma horizontal error (from the ellipse, covariance or circular error), `le` the vertical error. `9999999` when unknown. |
| `detail/track` | `course` (degrees true) and `speed` (m/s), when known. |
| `detail/contact` | `callsign`: the track's callsign, a `callsign` identifier, its name, its platform's name, or its track number (`OTK000000042`). |
| `detail/__security` | A labelled track only: `classification` (as the label gives it), `caveats` (its restrictions, `/` separated), `releasability` (its sharing) and `marking` (the portion marking, see [Markings](#markings)). Absent attributes are empty in the label. |
| `detail/remarks` | With **Remarks**: `OpenTrack <UID>; sources: <source ids>`. A labelled track's remarks start with its marking, `(S//REL TO USA, GBR) OpenTrack …`, and are the marking alone when **Remarks** is off, so every TAK client shows it. An unlabelled track has neither. |

A track that ends gets a delete, in the form ATAK sends:
`<event uid="tms-<UID>" type="t-x-d-d" how="m-g" …>` with `stale` equal to `time`, and
`<detail><link uid="tms-<UID>" relation="none" type="<its last type>"/><__forcedelete/></detail>`.
TAK removes the track at once. A delete carries no marking: it holds nothing about the track but
its uid. A client that missed it (disconnected at the time) drops the track
when it goes stale.

Updates of one track are sent at most every `OT_COT_MIN_INTERVAL_SECS` (2 s), identity and
classification changes at once. When the role starts it reads the live published tracks and sends
them to each output as it connects; a delete that happened while it was stopped is not sent, and
TAK drops that track when it goes stale.

### Status and metrics

The TAK output panel shows each output's state (connected, connecting or disconnected for a TAK
Server; listening, with the clients connected; sending for multicast; error when it cannot start,
such as a port in use or a missing certificate), the events sent (each client counts), errors (the
last one under the list and in the count's tooltip) and whether it is encrypted. The same comes from
`GET /api/v1/tak/status`. The role reports every 2 s; "No cot role is running" means none has for
15 s.

The Overview's throughput panel charts TAK events sent, connected clients, dropped clients and
errors. They are in `GET /api/v1/metrics` under `cot`: counters `sent`, `errors` and `dropped`,
each also per output as `sent:<name>` and so on, and gauges `clients`, `clients:<name>` and
`tracks`.

## Backup and restore

### What to back up

| What | Why | How |
|---|---|---|
| `opentrack.db` | Everything configured and decided. | [SQLite backup](#sqlite-backup), online. |
| `session.key` (or your `OT_SESSION_SECRET`) | Without it, every session and API token is void. | Copy it; keep it secret. |
| `profiles/trackers/` beside the database | Imported tracker profiles. | Copy it. |
| `.env` or wherever the `OT_*` settings live | The deployment's settings. | Copy it; it holds secrets. |
| Redis | The live picture and its track numbers. Optional, but see [Restore](#restore). | [Redis backup](#redis-backup). |
| External plugins | Only their address is in the database. | Back up their programs. |
| A [configuration export](#configuration-export) (optional) | The configuration in one file, to rebuild a node from. Not the track state or the audit record. | `opentrack config export --out <file>`; keep it as safe as the database. |

### SQLite backup

The database is in WAL mode, so a plain copy of `opentrack.db` while OpenTrack runs can miss
changes still in `opentrack.db-wal`, or be torn. Back it up online with SQLite itself, on the same
host (not over a network file system):

```sh
sqlite3 data/opentrack.db ".backup '/backups/opentrack-$(date +%Y%m%d-%H%M).db'"
```

or, with SQLite 3.27 or newer, a compacted copy:

```sh
sqlite3 data/opentrack.db "VACUUM INTO '/backups/opentrack-$(date +%Y%m%d-%H%M).db'"
```

Both take a consistent snapshot while OpenTrack goes on writing. The Docker image has no `sqlite3`;
run it on the host, against the mounted data directory. Check a backup with
`sqlite3 <backup> "PRAGMA integrity_check"`. Or stop OpenTrack and copy the three files
(`opentrack.db`, `-wal`, `-shm`) together.

### Redis backup

Redis persists itself: take a snapshot with `redis-cli BGSAVE` and copy its `dump.rdb` once
`LASTSAVE` changes, or run Redis with append-only files. That backs up the whole Redis, other
applications' keys included. Take it at the same time as the SQLite backup.

### Configuration export

**Settings → Data → Full configuration** (admins; `GET /api/v1/export/config`, or
`opentrack config export`) downloads the node's whole configuration as one JSON file, to back it
up or to [rebuild the node](#rebuild-a-node-from-a-configuration-export) from. Every export is
recorded (an `export_config` decision in the audit record, with who exported it and how much), and
the download is an attachment no cache keeps (`Cache-Control: no-store`).

**It holds secrets.** Source credentials, password hashes, the SAML and OpenStare settings and
plugin components are in it as stored. Keep it as safe as the database: an encrypted volume or a
vault, readable by admins only, and delete copies you no longer need. `opentrack config export
--out <file>` writes a file only its owner can read.

What it holds (the file's sections):

| Section | What |
|---|---|
| `format`, `version` | `"opentrack-config"`, `2`. Also `opentrack` (the release that wrote it), `site_code`, `exported_at`, `exported_by`, `notice`, `marking` (see [Markings](#markings)). |
| `sources` | Every source as stored: `id`, `name`, `transport`, `codec`, `enabled`, `priority`, `revision`, `spec` (with its secrets), `raw_subject` (raw output, if consented). |
| `schema_versions` | Every output schema version (`version`, `status`, `published_at_ms`, `notes`, `fields`), the draft too. |
| `correlation_settings` | The saved correlation settings, security labels included (`null`: never saved). |
| `app_settings` | The instance settings as saved (site name, banners, position history, nodes), `{}` if never saved. |
| `auth_settings` | The sign-in settings as saved: password sign-in, SAML, OpenStare trust, client certificate mappings, and the accounts inactivity never turns off. A file from 0.4.0 may also have password, lockout, session, inactivity-period and audit-retention settings: they import, and are ignored (those values are fixed). |
| `accounts` | Every account: `id`, `email`, `name`, `role`, `active`, `origin`, `password_hash`, `password_history`, `password_changed_at_ms`, `must_change_password`, `locked_until_ms`, `last_login_at_ms` and the other account dates. |
| `api_tokens` | Every API token's record: `jti`, `name`, `user_id`, `created_by`, `expires_at_ms`, `revoked_at_ms`. Not the tokens themselves, which are never stored. |
| `registry.entities` | Every entity as the registry API shapes it: identifiers, status, publish override, the OTH-GOLD minimum and attributes. |
| `plugins` | Every added plugin: `name`, `runtime`, `version`, `manifest`, `wasm` (the component, base64) and `sha256`, or `address` (external), `grants`, `enabled`. |
| `tracker_profiles` | The tracker profiles imported on this node (not the shipped ones), each as its file. |
| `uid_sequences` | The next track number per site code (`site`, `next_sequence`). |

What it never holds: the session signing key (`session.key` or `OT_SESSION_SECRET`), sessions,
the audit record, the decision log, the track graph, groups, correlation suggestions, entity and
source revision history, anything in Redis, and the `OT_*` deployment settings. API tokens are
signed with the session key, so the tokens in the file work again only on a node with the same
key. External plugins' programs are not in it (only their address).

Files from 0.4.0 and earlier (no `format`, version 1) held only sources, schema versions and
correlation and instance settings; they cannot be imported.

### Rebuild a node from a configuration export

An import rebuilds a node that has **no configuration yet**, and refuses any other, listing what is
there. A node is empty when it has:
- no sources, registry entities, plugins, API tokens or imported tracker profiles;
- no output schema version but the built-in version 1;
- no saved correlation, instance or sign-in settings (the defaults);
- at most one account, an admin: the first admin made on first start.
- never imported a configuration before.

Track state (tracks, other decisions, the audit record) does not count. On a new node:

```sh
opentrack config status                     # empty?
opentrack config import opentrack-config-OTK-20260929-1200.json
opentrack serve                              # or all; restart a running server
```

The CLI is the usual way: an empty node's only account is its first admin. From the UI, an admin
of an empty node (after changing the first admin's password) sees **Settings → Data →
Import configuration**; `POST /api/v1/import/config` with the file as the body does the same, and
`GET /api/v1/import/config` says whether the node is empty (`{"empty", "present"}`).

The import checks every section first, as the API checks it when saved: sources against the file's
output schema versions, each settings document by its own rules, accounts (unique emails, valid
roles, at least one active admin), tokens against the accounts, entities and their identifiers,
plugins (the component's SHA-256 must match), tracker profiles, track numbers. A malformed file,
another format or version, or any invalid section is refused with every problem listed, and
nothing is written. Then it writes everything in one SQLite transaction (tracker profile files
included, removed again if the transaction fails), recorded as one `import_config` decision.

- **Accounts:** the imported accounts replace the first admin (whatever its email), with their
  password hashes, history and flags, so everyone signs in with their old password.
  `initial-admin.txt` is deleted. A file with no accounts (from a node with sign-in off) keeps the
  first admin.
- **API tokens:** set the same `OT_SESSION_SECRET` (or copy `session.key`) for them to work;
  otherwise revoke them and issue new ones.
- **Raw output** of a source is consented again by the import decision.
- **Track numbers** continue from the file's counter (or this node's, if higher), so no number is
  issued twice. Keep the same `OT_SITE_CODE`; the import notes it if the file came from another
  site.
- Restart a server that was running during a CLI import, so it reads the new sign-in settings.

### Registry export

**Registry → Export XLSX** or **Export CSV** (any role; `GET /api/v1/registry/export?format=xlsx|csv`)
downloads every entity: id, name, status, publish override, the OTH-GOLD minimum, identifiers and
attributes. **Import sheet** (track managers) brings a sheet back: it plans every row first, and
writes nothing while any row has an error. A row updates the entity its `entity_id` names (and
creates it under that id if missing), else the one its identifiers belong to, else creates one.
The entities' revision history is not in the sheet. The sheet's first row is its marking (see
[Markings](#markings)); an import reads a sheet with or without it.

### Restore

1. **Stop every OpenTrack role.**
2. **Put the database back:** copy the backup to `OT_SQLITE_PATH`, and delete any
   `opentrack.db-wal` and `opentrack.db-shm` left from the old database. Put back `session.key`
   (or set the same `OT_SESSION_SECRET`) and `profiles/trackers/`.
3. **Redis:** restore the Redis snapshot taken with the database backup. If you have none, clear
   OpenTrack's keys, so the engine does not start from a picture the database doesn't know:
   ```sh
   redis-cli --scan --pattern 'tms:*' | xargs -r -n 500 redis-cli del
   ```
   (use your `OT_REDIS_NAMESPACE`). Other applications' keys stay.
4. **Track numbers are checked on start.** Track numbers come from a counter in the database, so
   a backup takes the counter back to its value then. Before issuing any number, the engine looks
   for the highest number of its site code already in use: in the database (track graph, decision
   log), in Redis (live tracks and their history) and in the NATS tracks stream (its subjects,
   `<prefix>.tms-<UID>`). If the counter is not past it, the engine moves it on (never back), logs
   a warning with the old and new values, and records a `uid_counter_advanced` decision saying
   where the number was found. If NATS cannot be reached then, the engine logs that it could not
   check the stream and relies on the database and Redis: start NATS first when restoring.
   Tracks this node published that are no longer live (Redis lost or cleared) would otherwise stay
   in the stream as consumers last saw them: the engine publishes a delete for each (and logs how
   many). Subjects of other site codes, or last written by another publisher, are left
   alone and age out (`OT_NATS_MAX_AGE_HOURS`).
5. **Start OpenTrack.** Migrations run if the backup is from an older release. Check the Overview
   tab's **System status**.

## Upgrades

1. **Read [CHANGELOG.md](../../CHANGELOG.md)** from your release to the new one. Each release lists
   what changed and, under **Upgrading**, what to do.
2. **Back up** the database and `session.key` ([SQLite backup](#sqlite-backup)).
3. **Stop OpenTrack, install the new release** (`docker compose up -d --build`, or the new binary
   and UI build), and start it. Database migrations run on start. To run them on their own first:
   `opentrack migrate`, which prints the new schema version.
4. **Check** the Overview tab: **System status** shows the version and the SQLite schema version,
   and every dependency should be green. Browsers load the new UI on their next page load.

With roles in separate processes, migrate once (or start one role) before starting the others.

A node still on the host-network compose file (0.4.3 and before) moves its data directory into a
volume and gets its own Redis when it takes the new `docker-compose.yml`: follow
[Upgrading a host-network deployment](#upgrading-a-host-network-deployment) instead of step 3.

To go back, restore the backup from step 2 and run the old release. An older release refuses a
database a newer one has migrated: "database schema version N is newer than this build supports
(M)".

OpenTrack is alpha: the published message, the plugin interface and the database schema may still
change between releases.

## Purge

**Settings → Data → Purge** (admins; `POST /api/v1/admin/purge`) clears the picture:
1. Select **Purge tracks**.
2. In the window that opens, optionally turn on **Also delete history**, then select **Purge**.

It then:
- dissolves every group;
- retires every live track, and deletes the published ones downstream;
- with **Also delete history**, also deletes the track graph (how every track was formed, paired,
  merged and split, and the do-not-pair rules between tracks) and every correlation suggestion.

It keeps sources, the output schema, the registry and its entities, settings, accounts, plugins
and the decision log (a purge is itself a decision). Sources keep reporting, so new tracks appear
at once, with new numbers. It cannot be undone.

The API call is `{"confirm": "<site code>", "history": true|false}`: the API still asks for the
site code, so a stray request can't purge. It waits up to 5 minutes for the engine.

## Monitoring

### Health checks

- **`GET /healthz`** answers `ok`, with no sign-in. It only says the control plane process
  answers. Use it for a container or load-balancer liveness check.
- **`GET /api/v1/status`** (any role; use a `viewer` API token; paths, URLs, endpoints and error
  text are for admins, other roles see only up or down) reports each dependency with an
  `ok` flag: `sqlite` (with its schema version), `redis`, and `nats` (connected, and the stream
  usable), plus the version, algorithm versions, site code and node id. It answers 200 when all
  three are up and 503, with the same body, when any is down, so a readiness check or monitor
  that reads only the status code sees it; the flags say which. It also reports `telemetry`
  ([OpenTelemetry](#opentelemetry)): `configured`, `ok`, the endpoints and each exporting role's
  last success and error. Telemetry is not counted in the 503, so a collector outage never makes
  an orchestrator restart OpenTrack.
- **`GET /api/v1/sync/status`**: the link to other nodes.

### Metrics

The **Overview** tab:
- **System status:** control plane, algorithms, SQLite, Redis and NATS, each green or red with
  its error; and **Telemetry**, green or red when [OpenTelemetry](#opentelemetry) export is on,
  grey (off) when it isn't.
- **Throughput** (per minute, last hour): ingest (frames, observations, dropped, errors),
  observations by source, correlation (applied, new tracks, paired, proposed or split, ended),
  output (tracks written, deletes, raw feed, errors), and, once the `cot` role has sent anything,
  TAK output (events, clients, dropped, errors; [TAK output](#tak-output)).
- **Tracks:** live, confirmed, tentative, lost, with an entity, entity differs, and per domain.
- **Backlog and resources:** CPU, memory, threads, Redis, SQLite and NATS stream size, the
  engine's and writer's backlogs, uptime.

A backlog that keeps growing means the engine or the writer can't keep up (or isn't running). The
engine can fall behind by at most `OT_OBS_WINDOW_SECS` before reports are lost.

The same figures come from `GET /api/v1/metrics?minutes=60` (up to 1440), and a source's from
`GET /api/v1/sources/<id>/metrics`. Each source's **Status** tab shows its state, last frame and
error.

### Logs

OpenTrack logs to standard output (`docker compose logs -f opentrack`) and, when an endpoint is
set, over [OpenTelemetry](#opentelemetry). Set `OT_LOG_FORMAT=json` for a collector that reads
standard output, and `OT_LOG` for more or less detail ([Logging](#logging)). Worth watching:
- `audit record` (target `audit`): every audit row, see [Audit record](#audit-record);
- `SAML sign-on refused` and `sign-in refused`;
- `OpenStare sign-in check failed`;
- a source's connection errors;
- `the engine did not answer`;
- `OpenTelemetry export failing`.

Every change a person makes is in the decision log, with who and when (the Track Management and
Correlation tabs show it; `GET /api/v1/decisions`).

## Troubleshooting

| Symptom | Likely cause and fix |
|---|---|
| A request fails with `internal error (reference 3fa1c2…)`. | The server log has the detail: search it (or the OpenTelemetry logs) for the reference. |
| Every page answers `accept the notice first`. | The notice-and-consent warning is on and this sign-in hasn't accepted it: reload the page and choose AGREE. A script should use an API token. |
| A file source is refused: `a file source reads only under the data directory`. | Put the files under the data directory (`/data` in the image) and give that path, or a path relative to it. |
| The server doesn't start: `OT_ADMIN_PASSWORD: the password needs at least 15 characters, ...`. | Choose a password that meets the [password policy](#passwords), or leave both `OT_ADMIN_*` unset. |
| No `initial-admin.txt`. | It is made only on the first start with no accounts and no `OT_ADMIN_*`. Use `opentrack user add <email> --role admin --password-stdin`. |
| Every admin is locked out. | `opentrack user passwd <email>` (or `user enable`, or `user add … --role admin`) on the server. |
| Everyone was signed out after a restart, and API tokens fail. | The signing key changed: `session.key` was lost or `OT_SESSION_SECRET` changed. Restore it, or make new tokens. |
| "Too many attempts". | 5 sign-in attempts at once per address, then one a second. Wait. |
| "Single sign-on failed". | Read `SAML sign-on refused` in the log: no `OT_PUBLIC_URL`, a response not signed or with the wrong certificate (paste fresh metadata), no email in the NameID, no role mapped and no default, the account is off, or more than 5 minutes between request and answer. |
| "sign in with single sign-on before turning password sign-in off". | See [Password sign-in](#password-sign-in). |
| An action fails with "the engine did not answer within 10 s (is it running?)". | The engine isn't running (a lone `serve`), or is stuck: check its log and the engine backlog. |
| "database schema version N is newer than this build supports". | A newer release migrated this database. Run that release, or restore the pre-upgrade backup ([Upgrades](#upgrades)). |
| NATS is red in System status. | Not connected (check `OT_NATS_URL` and credentials), or the stream is unusable: its error is shown. OpenTrack creates the stream only if it is missing. |
| A source stays "connecting" or "failing". | Its **Status** tab shows the error. Check the address, TLS files and `${env:…}` secrets. |
| Tracks are live but consumers don't get them. | See why each is not published on its track card (operator guide, "Not published"): not confirmed, only sensors that may not stand alone report for it, the output filter holds it, or an entity says never. |
| `docker compose up` refuses to start. | `.env` is missing next to `docker-compose.yml`. |
| The container exits at once. | The data directory isn't writable by uid 1000 (see its log). |
| Redis memory keeps growing. | Position history (Settings → General), `OT_OBS_WINDOW_SECS`, and the number of tracks. See [Redis](#redis). |

## Security

- **Keep sign-in on.** `OT_AUTH=off` makes everyone an admin.
- **Changes come from OpenTrack's page.** A change (anything but a read) made with the session
  cookie must carry the `X-OpenTrack-CSRF` header the page sends with every call, and any change
  whose `Origin` names another site is refused (403, audited as `access_denied` with reason
  `csrf`). Scripts with an API token need no header. The server is the address a browser asked
  for (`Host`), the one a proxy says it was asked for (`X-Forwarded-Host`), or `OT_PUBLIC_URL`.
  SAML's assertion consumer is the one address another site (the identity provider) posts to.
- **Request rate.** Each client may make 20 API requests a second, in bursts of up to 100: an
  account (all its sessions together), each API token on its own, or an address before sign-in.
  More is answered 429 with `Retry-After`, and the first in a minute is logged and audited
  (`rate_limited`). A busy page makes a few a second. Sign-in has its own stricter limit. The
  values are fixed.
- **Content security policy.** The page may run only this server's scripts, and styles from
  this server's stylesheets or a `<style>` carrying the page's nonce, fresh on each load:
  injected markup can neither run nor style anything.
- **Use TLS** (`OT_TLS_CERT`, `OT_TLS_KEY`), or a TLS-terminating proxy that forwards to OpenTrack
  on a private address. Over TLS, session cookies are `Secure`.
- **Protect the data directory.** `session.key` and `initial-admin.txt` are written readable by
  their owner only; the database holds password hashes and plugin files. Keep backups as
  protected.
- **Keep secrets out of source specs.** Use `${env:NAME}`. Non-admins see secret fields hidden,
  but admins, the database and its backups hold whatever is written inline.
- **Give the least role.** Viewers for monitoring and service accounts that only read; admins
  few. Leave **Allow admin** off for SAML unless the identity provider should make admins.
- **Local accounts and SAML:** SAML never signs in to a local account: a sign-on whose email matches
  one is refused. Keep local accounts for break-glass and service use.
- **Rotate API tokens**: give them an expiry, one per service, and revoke the ones not used.
- **Everything is recorded.** The decision log names the account behind every change: accounts,
  sign-in settings, sources, schema, settings and track management.

<a id="security-hardening"></a>

### Security hardening (0.4.0)

0.4.0 is the accreditation release, aimed at DoD RMF with the ASD STIG and NIST 800-53 Moderate.
Each control below is always on, at the STIG value: the password policy, lockout, session limits,
inactivity and audit retention are fixed, not settings. What an admin sets is the break-glass
accounts (**Never turn off** in Settings → Users) and sign-in itself (the **Single sign-on**
panel). How each one maps to a control is in
[docs/security/stig-mapping.md](../security/stig-mapping.md). Deployment steps are in
[docs/security/hardening.md](../security/hardening.md).

### FIPS cryptography

All of OpenTrack's cryptography runs in FIPS 140-3 validated modules. That covers TLS, session and
token signatures, password hashes and random secrets (the AWS-LC FIPS module), and SAML
signatures (OpenSSL's 3.0.9 FIPS provider, in the image). The process stops at start if AWS-LC
is not in FIPS mode. SAML sign-in is off wherever OpenSSL is not, which means everywhere outside
the image. Details: [docs/security/fips.md](../security/fips.md).

### Password policy

For local accounts:

| Rule | Value |
|---|---|
| Minimum length | 15 |
| Upper case, lower case, digit, special character | all required |
| Characters that must change from the last password | 8 |
| Earlier passwords that can't be reused | 5 |
| Minimum age (between changes) | 24 hours (an admin's reset is exempt) |
| Maximum age | 60 days |
| Common passwords | refused: one of the 100,000 most common, or one whose letters spell one (`Password2026!!!`) |

The common-password list is built in (SecLists, MIT licence). To add your own, such as
passwords known to be compromised in your organisation, put one per line in
`common-passwords.txt` in the data directory (beside `opentrack.db`; lines starting with `#` are
skipped); it is read at the next start.

An expired password, or a temporary one an admin set, must be changed at the next sign-in. Until
it is, that session can do nothing else. The first admin's password is temporary as well. That
includes the one in `initial-admin.txt`, and `OT_ADMIN_PASSWORD` must already meet the policy.

### Account lockout

Three failed sign-ins within 15 minutes lock an account until an admin unlocks it (fixed; the
Container Platform SRG's rule, stricter than the ASD STIG's timed lock). Every refusal gives the
same answer, "wrong email or password", whether the account is unknown, turned off, locked or
the password is wrong. The audit record keeps the real reason (`account_locked`). To unlock an
account:
- **Settings → Users:** the open-lock button on its row;
- **command line:** `opentrack user unlock <email>`, on the server, which works when every admin
  is locked out;
- **API:** `POST /api/v1/auth/users/{id}/unlock`.

### Session limits

- **Idle timeout:** 15 minutes, or 10 for admins. The page's own refreshing doesn't count as use;
  only what the user does.
- **Absolute lifetime:** 24 hours, used or not.
- **Sessions per account:** 3. A fourth sign-in ends the oldest.

Users see and end their own sessions from the account menu, under **Sessions**. An admin ends all
of an account's sessions with **Sign out everywhere** on its row in Settings → Users
([Revoking sessions](#revoking-sessions)). Sessions are kept per node. API tokens aren't
sessions: they have no idle timeout, only their expiry.

### Inactive accounts

Accounts that haven't signed in for 35 days are turned off. The check runs at sign-in and every
10 minutes. Re-enabling an account restarts its clock. Mark break-glass accounts with **Never
turn off** on their row in Settings → Users (keep them few, with sealed passwords). Otherwise a
sole admin who doesn't sign in for 35 days is turned off too, and only
`opentrack user enable <email>` on the server brings the account back.

After each sign-in, users see when they last signed in and how many failed attempts there were
since then.

### Audit record

Every sign-in event is written to an append-only `audit` table, along with every decision the
decision log records:
- sign-in succeeded or refused, with the reason and address, and the first use in an hour of each
  API token, client certificate and OpenStare identity (`login`, `via`);
- sign-out, lockout, unlock, session ended or timed out, the notice accepted
  (`consent_accepted`);
- password changed, account turned off (inactive or expired);
- a credential that identifies no one (`access_refused`), a role that doesn't reach
  (`access_denied`), a change from another site's page (`access_denied`, reason `csrf`), a
  change the server refused (`change_refused`, with the status);
- a client over the request rate (`rate_limited`, once a minute per client);
- a client certificate refused as revoked or of unknown status (`certificate_refused`), and an
  OCSP query with no usable answer, so the revocation lists decided (`ocsp_unavailable`);
- an admin reading accounts, API tokens, sign-in settings, the configuration export or the
  audit record (`read_security_object`), and every source probe (`probe_source`).

A decision's row carries the address of the request that made it.

Each row carries a SHA-256 over the row before it, so a row changed, removed or inserted breaks
the chain. The database refuses updates to the table.
- **Review in your SIEM:** every row is also logged, once written, as an `audit record`
  event (target `audit`) with its `seq`, `actor`, `op`, `outcome`, `ip`, `detail`, `decision_id`
  and `hash`. Over [OpenTelemetry](#opentelemetry) it is a log record with event name
  `audit.record` and those fields as attributes; on standard output, set `OT_LOG_FORMAT=json`. A decision's `before` and
  `after` (the configuration it changed, which can hold source credentials) are left out of the
  log; its `decision_id` finds them in the API. For example:

  ```json
  {"timestamp":"2026-09-29T14:02:11.418220Z","level":"INFO","fields":{"message":"audit record","seq":4182,"actor":"ann@example.org","op":"login","outcome":"failure","ip":"10.1.2.3","detail":"{\"consecutive\":2,\"reason\":\"bad_password\",\"via\":\"password\"}","hash":"55e122e2f66b7312482b6dd6624afe22e3c4f0d6f1b44c29cb1333c1a32c5d84"},"target":"audit"}
  ```
- **Review through the API** (admins): `GET /api/v1/audit` filters by time (`from_ms`, `to_ms`),
  account (`actor`), event (`op`, comma-separated) and `outcome`, pages back with `before_seq`,
  and gives CSV with `format=csv` (its first line the banner's text as the file's marking, see
  [Markings](#markings)). `GET /api/v1/audit/verify` checks the chain.
- **Fail closed:** if a sign-in can't be recorded in the table, it is refused. A collector outage
  does not refuse anything: the table has the record, and the Telemetry status shows the outage
  ([OpenTelemetry](#opentelemetry)).
- **Retention:** kept forever. Nothing deletes rows.
- **Chain head:** written to the log every hour. Keep the logs apart from the database, so that a
  truncated tail can be spotted.

### Web protections

Every response carries:
- a Content Security Policy (this origin only, no framing);
- `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer` and a
  minimal `Permissions-Policy`;
- HSTS over TLS;
- `Cache-Control: no-store` on the API.

The session cookie is `HttpOnly`, `SameSite=Strict`, and `Secure` whenever TLS is in use,
including TLS ended at a proxy (`OT_PUBLIC_TLS=1` or an `https://` `OT_PUBLIC_URL`).

### Encryption in transit

| Link | How |
|---|---|
| Browsers and API clients | `OT_TLS_CERT`/`OT_TLS_KEY`, optional client certificates (`OT_TLS_CLIENT_CA`, revocation lists `OT_TLS_CLIENT_CRL`), or a TLS proxy |
| NATS | a `tls://` URL, `OT_NATS_CA`, and mutual TLS with `OT_NATS_CERT`/`OT_NATS_KEY` |
| `opentrack bridge` to each node's NATS | the same, as `OT_BRIDGE_NATS_CA`, `OT_BRIDGE_NATS_CERT`/`OT_BRIDGE_NATS_KEY`, or per node ([The bridge's NATS credentials and TLS](#the-bridges-nats-credentials-and-tls)) |
| Redis | a `rediss://` URL, `OT_REDIS_CA`, and mutual TLS with `OT_REDIS_CERT`/`OT_REDIS_KEY` |
| Feeds | per source, in its transport's TLS settings |
| TAK | per output in Settings → TAK output: TLS to a TAK Server with a client certificate, TLS for a listening output with optional client certificates and revocation lists. Multicast is always plaintext ([TAK output](#tak-output)) |

### Container hardening

The image runs as an unprivileged user (uid 1000) and has a health check (`opentrack health`).
Its runtime base is distroless Debian 12 (`gcr.io/distroless/cc-debian12`): no shell, no package
manager, only glibc, OpenSSL 3, CA certificates and the shared libraries the binary loads (SAML's
libxmlsec1 and libxml2 and theirs). Its base images are pinned by digest. The compose file runs it
with:
- a read-only root file system (only `/data` and scratch space are writable);
- every Linux capability dropped;
- `no-new-privileges`;
- a bridge network with only port 8090 published, and Redis on an internal network;
- a named volume for `/data`;
- memory, CPU and process limits ([Docker compose](#docker-compose)).

Redis in the compose file runs the same way: its own user, read-only, no capabilities,
`no-new-privileges`, limits, and no published port.

### Signed images

Release images (`ghcr.io/kineticlogic-io/opentrack:<version>`) are signed with the project's cosign
key and carry SLSA provenance and an SBOM. Check one before you run it:

```sh
cosign verify --key cosign.pub --insecure-ignore-tlog=true ghcr.io/kineticlogic-io/opentrack@<digest>
```

`cosign.pub` is in the repository; the release notes list each image's digest.
`--insecure-ignore-tlog` is expected: the signatures go to no public transparency log.

### Upgrading to 0.4.0

- **Everyone signs in again:** sessions from before the upgrade have no server-side record.
- **Password age** counts from the upgrade, so existing passwords expire 60 days later. They also
  must meet the policy at their next change.
- **Password hashes** move from Argon2id to PBKDF2 at each account's next sign-in.
- **SAML** works only in the image, where OpenSSL has its FIPS provider, and no longer signs in to
  local accounts: a user with both needs a different email for the local one.
- **Nodes that share their profile** must all run 0.4.0. Older nodes refuse correlation settings
  that carry the classification order.
