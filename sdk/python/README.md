# Python plugin SDK

```sh
pip install -e sdk/python                 # add [component] to build WebAssembly components
```

Subclass `Codec`, `Tracker` or `Scorer`, describe them in a `Plugin` and call `main(PLUGIN)`. The
module then runs two ways:

- `python mine.py serve --address 127.0.0.1:47300` serves it as an **external plugin**, with any
  packages it imports (numpy, Stone Soup, a GPU). Add the address in Settings → Plugins with the
  secret the plugin and OpenTrack share: *Generate* makes one there; start the plugin with it in
  `OT_PLUGIN_SECRET` or a file given with `--secret-file`. Every connection proves both ends hold
  it before anything else. Without a secret the plugin is reached only over a unix socket under
  OpenTrack's data directory.
- `python mine.py build -o mine.wasm` builds a **WebAssembly component** that runs sandboxed inside
  OpenTrack. This works for plain Python only (the standard library and pure-Python packages), and
  needs componentize-py.

`python mine.py describe` prints the manifest. Plots, tracks and reports are dicts in OpenTrack's
observation schema; `track(plot, key, lat, lon, course=, speed=)` makes a track report from a plot.

The examples:
- [domain_scorer.py](examples/domain_scorer.py): plain Python; builds as a component
- [stonesoup_tracker.py](examples/stonesoup_tracker.py): a Stone Soup GNN tracker, external

The interface: [docs/plugins.md](../../docs/plugins.md).
