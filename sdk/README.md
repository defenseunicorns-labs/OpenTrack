# Plugin SDKs

Write OpenTrack codecs, trackers and pairing scorers of your own. See
[docs/plugins.md](../docs/plugins.md) for the interface, grants and the external protocol.

| SDK | Builds | Examples |
|---|---|---|
| [rust/](rust) | WebAssembly components (`cargo build --release --target wasm32-wasip2`) | `alpha-beta` tracker, `csv-reports` codec, `speed-veto` scorer |
| [python/](python) | external plugins (`python mine.py serve`) or, for plain Python, components (`python mine.py build -o mine.wasm`) | `domain-scorer`, `stonesoup-gnn` tracker |

Add a plugin in Settings → Plugins, or check it first with `opentrack plugin check <file or address>`.

An external plugin shares a secret with OpenTrack, and the two prove they hold it on every
connection ([Authentication](../docs/plugins.md#authentication)). The Python SDK's `serve` does
this: give it the secret in `OT_PLUGIN_SECRET` or a file named with `--secret-file` (never on the
command line). Rust plugins are components, which need no secret; a Rust program serving the
external protocol itself can use `ot_plugin::handshake` from this repository for the proofs.
