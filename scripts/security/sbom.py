#!/usr/bin/env python3
"""CycloneDX SBOMs for a release: the server binary and the UI bundle.

cargo-cyclonedx reads `cargo metadata`, which unifies features across every
target, so it lists crates this build never compiles (e.g. `ring` via a
WebAssembly-only edge). This keeps only the crates `cargo tree` says are
built into the binary for the host target, and records that it did so.

    scripts/security/sbom.py [out-dir]    (default: target/sbom)

Needs cargo-cyclonedx (`cargo install --locked cargo-cyclonedx`) and npm.
"""
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
OUT = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ROOT / "target" / "sbom")


def run(*args, cwd=ROOT, **kw):
    return subprocess.run(args, cwd=cwd, check=True, text=True, capture_output=True, **kw).stdout


def built_crates():
    """(name, version) of every crate compiled into the opentrack binary."""
    out = run("cargo", "tree", "-p", "ot-server", "-e", "normal,build",
              "--prefix", "none", "-f", "{p}")
    crates = set()
    for line in out.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[1].startswith("v"):
            crates.add((parts[0], parts[1][1:]))
    return crates


def server_sbom():
    run("cargo", "cyclonedx", "--format", "json", "--spec-version", "1.5",
        cwd=ROOT / "crates" / "ot-server")
    src = ROOT / "crates" / "ot-server" / "ot-server.cdx.json"
    bom = json.loads(src.read_text())
    src.unlink()
    keep = built_crates()
    dropped = [c["name"] for c in bom["components"] if (c["name"], c["version"]) not in keep]
    bom["components"] = [c for c in bom["components"] if (c["name"], c["version"]) in keep]
    refs = {c["bom-ref"] for c in bom["components"]} | {bom["metadata"]["component"]["bom-ref"]}
    bom["dependencies"] = [
        {**d, "dependsOn": [r for r in d.get("dependsOn", []) if r in refs]}
        for d in bom.get("dependencies", []) if d["ref"] in refs
    ]
    bom["metadata"].setdefault("properties", []).append({
        "name": "opentrack:filtered",
        "value": "components limited to crates compiled into the binary (cargo tree); "
                 "not built: " + ", ".join(sorted(set(dropped))),
    })
    return bom


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    bom = server_sbom()
    (OUT / "opentrack.cdx.json").write_text(json.dumps(bom, indent=2))
    ui = run("npm", "sbom", "--omit", "dev", "--sbom-format", "cyclonedx", cwd=ROOT / "ui")
    (OUT / "opentrack-ui.cdx.json").write_text(ui)
    print(f"{OUT}/opentrack.cdx.json: {len(bom['components'])} crates")
    print(f"{OUT}/opentrack-ui.cdx.json: {len(json.loads(ui).get('components', []))} packages")
    banned = {"ring", "openssl-src", "native-tls"} & {c["name"] for c in bom["components"]}
    if banned:
        sys.exit(f"banned crates in the build: {', '.join(sorted(banned))}")


if __name__ == "__main__":
    main()
