# Rust plugin SDK

`opentrack-plugin` turns Rust types into an OpenTrack plugin component. Implement `Codec`,
`Tracker` or `Scorer` (plots and tracks are `ot-core`'s `Observation`), name them in `plugin!`
with a manifest, and build for WebAssembly:

```sh
rustup target add wasm32-wasip2
cargo build --release --target wasm32-wasip2
ls target/wasm32-wasip2/release/*.wasm
```

`opentrack_plugin::log::{debug, info, warn, error}` write to OpenTrack's log.

Examples: [alpha-beta-tracker](examples/alpha-beta-tracker) (nearest neighbour with an alpha-beta
filter), [csv-codec](examples/csv-codec) (`key,time,lat,lon[,course,speed]` lines) and
[speed-scorer](examples/speed-scorer) (OpenTrack's kinematic evidence with a speed veto).
The interface: [docs/plugins.md](../../docs/plugins.md).
