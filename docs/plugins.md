# Plugins

A plugin adds a codec, a tracker or a pairing scorer to OpenTrack without changing OpenTrack.
One plugin may provide any of the three:

| Kind | What it does | Where it is used |
|---|---|---|
| **codec** | a frame's bytes to JSON records a mapping reads | a source's codec: `{"type": "plugin", "plugin": "<name>", "options": {...}}` |
| **tracker** | plots in, tracks out | a source's tracker stage: `{"algorithm": "plugin", "plugin": "<name>", "options": {...}}` |
| **scorer** | evidence whether a report and a system track are the same object | correlation settings: `"scorer": {"plugin": "<name>", "options": {...}}` |

The interface is [`wit/plugin.wit`](../wit/plugin.wit), a WebAssembly component interface.
A plugin runs one of two ways, and the engine cannot tell them apart:

- **WebAssembly**: a component that OpenTrack runs inside its own process with wasmtime. It is
  compiled to machine code on load and runs at close to native speed. It is sandboxed: it has no
  files, network or environment unless an operator grants them, and has a memory ceiling and a time
  limit per call. It is uploaded in Settings → General → Plugins and loaded without a restart. Write it in
  Rust ([sdk/rust](../sdk/rust)) or plain Python ([sdk/python](../sdk/python)).
- **External**: a program of its own that serves the same interface over a socket. It runs where
  you start it, with whatever it needs: numpy, scipy, Stone Soup, a GPU, a licensed library.
  Settings → General → Plugins registers its address and the secret it shares with OpenTrack (see
  [Authentication](#authentication)). The Python SDK serves any plugin this way.

## What a plugin sees

Values cross the interface as JSON, so OpenTrack's schema can grow without a new interface version.

- **Observations** (plots, tracks, reports, system track views) are OpenTrack's `Observation`:
  - `source_id`, `source_track_key`
  - `observed_at` (ISO 8601)
  - `position` `{latitude, longitude, altitude_hae_m}`
  - `uncertainty` `{circular_error_m, ellipse: {semi_major_m, semi_minor_m, orientation_deg}}`
  - `kinematics` `{course_deg, speed_mps, heading_deg, vertical_rate_mps}`
  - `classification` `{domain, affiliation, ...}`
  - `identifiers`, `name`, `callsign`, `state`, `track_type`, `ext`

  Rust plugins get the typed struct from `ot-core`. A tracker's output only needs a
  `source_track_key` and a position; the easiest way is to copy the plot that last updated the
  track and change those. OpenTrack sets `source_id` and stamps
  `provenance.tracker = "<plugin>-<version>"`.
- **Options** are what the source or the settings give, starting from the defaults in the
  manifest.
- **Times** are milliseconds since the Unix epoch.

### The manifest

`describe` returns:

```json
{
  "name": "alpha-beta",              // lowercase letters, digits, - and _
  "version": "1",                    // recorded with what the plugin produces
  "description": "…",
  "kinds": ["tracker"],              // the SDKs fill this in
  "options": [
    {"name": "gate_m", "label": "Gate (m)", "type": "number", "default": 100, "unit": "m", "min": 0,
     "help": "Plots further than this from a track's prediction start a new track"}
  ],
  "framing": {"type": "delimiter", "delimiter": "\n"}   // codecs: how to split a byte stream
}
```

Option types are `bool`, `number` (`unit`, `min`), `choice` (`choices`) and `text`. The UI draws
a form from them wherever the plugin is picked.

### Codec

`open-decoder(options)` starts one stream's decoder. Its state lives between frames. Two calls:

- `decode(frame, received-at-ms)` returns the frame's records.
- `hints()` may report what the stream has shown about the sensor
  (`{"revisit_secs", "revisit_source"}`). A tracker stage with `auto_timing` sizes itself by it.

### Tracker

`open-tracker(options)` starts one source's tracker. Two calls:

- `push(plot, received-at-ms)` hands it each plot.
- `run(now-ms, force)` returns the tracks to report now. It is called after every frame and about
  once a second, and with `force` at the end of a stream.

The plugin decides what a scan is and when a track is confirmed. A track ends with a report whose
`state` is `dropped`. Keep to OpenTrack's tracker rule: at most unknown affiliation and a domain,
never an identity.

### Scorer

`open-scorer(options)` starts a scorer. For every report, the engine calls
`score(report, candidates)` with each system track nearby that could be the same object. Each
candidate carries:

- `view`: the system track as it stands
- `kinematic`: OpenTrack's own comparison (`distance_m`, `dt_s`, `sigma_m`, `d2`, `dof`, `ln_lr`,
  `pass`)

The plugin answers one `{"ln_lr", "pass", "evidence"}` per candidate. These take the kinematic
comparison's place in the engine's sequential test. The test itself stays the engine's: prior,
m-of-n, pair probability, suggest mode, do-not-pair and splits. So does every decision, which
records the plugin, its version and its evidence. If a scorer fails, the kinematic comparisons
stand, and the failure is logged once.

## Grants

WebAssembly plugins only; set per plugin in Settings → General → Plugins or `PUT /api/v1/plugins/{name}`:

| Grant | Default | |
|---|---|---|
| `memory_mb` | 256 | most memory the plugin may grow to |
| `call_timeout_ms` | 5000 | a call running longer is stopped; the instance is not used again |
| `network` | none | `host:port` addresses it may connect to |
| `dirs` | none | `{path, guest?, write?}`: server directories it can see (read only unless `write`) |
| `env` | none | environment variables it sees |

Every decoder, tracker or scorer that is opened gets its own instance, with its own memory, so one
stream cannot disturb another. For an external plugin only the call timeout applies: it has what
its own process has.

## Writing one

**Rust** ([sdk/rust](../sdk/rust)): implement `Codec`, `Tracker` or `Scorer` and name them in
`plugin!`:

```rust
use opentrack_plugin::{Observation, Tracker, Value, json, plugin};

struct Mine { /* ... */ }

impl Tracker for Mine {
    fn open(options: Value) -> Result<Self, String> { /* ... */ }
    fn push(&mut self, plot: Observation, received_at_ms: i64) { /* ... */ }
    fn run(&mut self, now_ms: i64, force: bool) -> Result<Vec<Observation>, String> { /* ... */ }
}

plugin! {
    manifest: json!({"name": "mine", "version": "1", "description": "…", "options": []}),
    tracker: Mine,
}
```

```sh
cd sdk/rust && cargo build --release --target wasm32-wasip2   # rustup target add wasm32-wasip2
```

The examples are an alpha-beta tracker, a CSV codec and a speed-veto scorer.

**Python** ([sdk/python](../sdk/python)): subclass `Codec`, `Tracker` or `Scorer`, describe them
in a `Plugin`, and call `main(PLUGIN)`:

```sh
OT_PLUGIN_SECRET=… python mine.py serve --address 127.0.0.1:47300   # external (any packages)
python mine.py build -o mine.wasm      # as a component (plain Python; componentize-py)
```

The examples are a domain and course scorer (plain Python, builds as a component), and a Stone Soup
GNN tracker (external).

## Adding, checking and benchmarking

- **Check before adding**: `opentrack plugin check mine.wasm` (or an address) loads the plugin,
  opens each kind it provides with its default options, and prints what worked. Settings → General → Plugins
  has the same check. For an external plugin it reads the secret from `--secret-file` or
  `OT_PLUGIN_SECRET`.
- **Add**: Settings → General → Plugins → Add plugin, `POST /api/v1/plugins` (the component with
  `Content-Type: application/wasm`, or `{"address", "secret"}`), or `opentrack plugin add`.
  - A plugin is loaded before it is stored.
  - A new build replaces an old one only when asked (`?replace=true`). It keeps its grants and
    whether it is enabled.
  - Every role picks up changes within 5 s.
  - Every change goes in the decision log, with the component's SHA-256.
- **Delete** is refused while a source or the correlation settings use the plugin, unless forced.
- **Benchmark**: `scripts/benchmark/bench run` scores a plugin tracker against the built-in ones:

  ```sh
  scripts/benchmark/bench run 'autoferry-*' --plugin mine.wasm \
    --set '*.pipeline.tracker={"algorithm":"plugin","plugin":"mine","options":{}}'
  ```

## The external protocol

JSON lines over TCP (`host:port`, `tcp://host:port`) or a Unix socket (`unix:/path`). Each request
is `{"id", "method", "params"}`. Each reply is `{"id", "result"}` or `{"id", "error"}`.

Every decoder, tracker or scorer OpenTrack opens is one connection. It starts with `open`, and the
instance lives as long as the connection.

| Method | Params | Result |
|---|---|---|
| `describe` | | the manifest |
| `open` | `kind`, `options` | null |
| `decode` | `frame` (base64), `received_at_ms` | records |
| `hints` | | hints or null |
| `push` | `plot`, `received_at_ms` | null |
| `run` | `now_ms`, `force` | tracks |
| `score` | `report`, `candidates` | scores |

A reply slower than the plugin's `call_timeout_ms` fails the call and closes the session.
`opentrack_plugin.external` in the Python SDK is a complete server, handshake included.

### Authentication

Anything that can reach a plugin's socket could otherwise pose as OpenTrack (and read what it is
sent), and anything that can listen on the address could pose as the plugin (and feed OpenTrack
tracks or scores). So OpenTrack and every external plugin share a secret, and **each connection**
(the one that reads the manifest, and every decoder, tracker or scorer session) proves both ends
hold it before anything else is sent:

```text
→ {"id":1,"method":"hello","params":{"protocol":"opentrack-plugin-v1","challenge":"<Nc>"}}
← {"id":1,"result":{"proof":"<plugin proof>","challenge":"<Np>"}}
→ {"id":2,"method":"verify","params":{"proof":"<OpenTrack proof>"}}
← {"id":2,"result":null}
```

- `Nc` and `Np` are 32 random bytes each, standard base64. OpenTrack's come from the FIPS module's
  DRBG.
- plugin proof = HMAC-SHA256(secret, `"opentrack-plugin-v1 plugin"` ‖ `0x00` ‖ Nc ‖ Np);
  OpenTrack proof = HMAC-SHA256(secret, `"opentrack-plugin-v1 opentrack"` ‖ `0x00` ‖ Nc ‖ Np). The
  key is the secret's UTF-8 bytes. Both are checked in constant time.
- The plugin proves itself first; OpenTrack answers only once that proof checks, so it never signs
  anything for a plugin that does not hold the secret. The role labels mean neither side's proof
  can pass as the other's, so a challenge reflected back fails.
- A plugin answers nothing but `hello` and `verify` until OpenTrack's proof checks, and hangs up
  if it does not.

OpenTrack refuses a plugin that answers `hello` with an error, does not answer, or proves the wrong
secret. It logs the failure as an error and records `plugin_auth_failed` in the audit record, and
the plugin's status in Settings → General → Plugins says why.

**The secret.** In Add plugin (or a plugin's key button), *Generate* makes one: 32 random bytes from
the FIPS DRBG, as hex. It is shown that once; give it to the plugin, then add or save. You can
paste your own instead (at least 32 characters), or `${env:NAME}` to read it from OpenTrack's
environment when connecting. OpenTrack keeps it in its database with the plugin (it needs the
secret itself to answer the plugin's challenge), like a source's credentials; the API only says
whether one is set, and the configuration export carries it. Start the plugin with it in
`OT_PLUGIN_SECRET`, or in a file named with `--secret-file` (not on the command line, where other
accounts can see it).

**Without a secret** OpenTrack connects only to a unix socket under its data directory (the
directory holding its database, links resolved). Only accounts that can write that directory can
listen there, so the filesystem already says who the plugin is; OpenTrack skips the handshake. Any
other address with no secret is refused before connecting.

The handshake authenticates both ends when a connection opens. It does not encrypt what follows or
stop it being changed on the way: keep external plugins on a unix socket, the same host or a network
you trust.

A program in another language can serve the protocol too: `ot_plugin::handshake` (Rust) and
`opentrack_plugin.external.proof` (Python) compute the proofs.

**Upgrading from 0.4.4 or earlier.** External plugins had no secret. After the upgrade one without
a secret shows status *error: no secret…* and does not load (unless it is on a unix socket under the
data directory). For each: update the plugin to this SDK, *Generate* a secret for it in
Settings → General → Plugins, start the plugin with that secret, then save it. Sources and the
correlation scorer that use it pick it up within 5 s.
