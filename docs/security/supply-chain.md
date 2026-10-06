# Supply chain

This covers what goes into an OpenTrack build and how that is checked.

## Pinned inputs

| Input | How it is pinned |
|---|---|
| Rust crates | `Cargo.lock`, built with `--locked` |
| UI packages | `ui/package-lock.json`, installed with `npm ci` (stareSDK is `@kineticlogic/staresdk` from npm) |
| Forked crates | `third_party/samael` (SAML) and `third_party/rumqttc` (MQTT; FIPS patch). Each has a note on what differs from upstream |
| Base images | Pinned by digest in the `Dockerfile` (below) and, for Redis and the development NATS, in `docker-compose.yml` |
| Go (FIPS module build) | Version and SHA-256 checked in the `Dockerfile` |
| OpenSSL FIPS provider | Release 3.0.9 source, SHA-256 checked in the `Dockerfile` |
| CI actions | Pinned by commit SHA in `.github/workflows/*.yml` |

**Base images** (`Dockerfile`):

| Image | Stage | What it is for |
|---|---|---|
| `node:22-bookworm-slim` | `ui` | Builds the UI; nothing of it ships |
| `debian:bookworm-slim` | `openssl-fips` | Builds the OpenSSL 3.0.9 FIPS provider; only `fips.so` and `fipsmodule.cnf` ship |
| `rust:1-bookworm` | `server` | Builds the binary |
| `debian:bookworm-slim` | `runtime-libs` | Supplies the shared libraries the binary needs that the runtime base lacks (found with `ldd`: libxmlsec1, libxmlsec1-openssl, libxml2, libxslt, ICU, zlib, liblzma), with their dpkg records and copyright files |
| `gcr.io/distroless/cc-debian12:nonroot` | runtime | The image itself: glibc, libgcc, libstdc++, OpenSSL 3 (`libssl3`), CA certificates, tzdata. No shell, no package manager |

The libraries copied in keep their Debian package records (`/var/lib/dpkg/status.d/`, as distroless
records its own), so an image scanner and the SBOM still list them by package and version.

**Moving a base image:** run
`docker buildx imagetools inspect <image>:<tag>` and replace the digest after
the `@`. Rebuild and run the tests, then note the change in CHANGELOG.

## Checks in CI

- **`cargo deny check`** (`deny.toml`):
  - known vulnerabilities (RustSec);
  - licences against an allow-list;
  - crates banned for FIPS (`ring`, `openssl-src`, `native-tls`);
  - dependencies only from crates.io.
- **`npm audit --omit=dev --audit-level=high`** on the UI's runtime
  dependencies.
- **SBOMs**, CycloneDX JSON, attached to every CI run as the `sbom` artifact:
  - `opentrack.cdx.json`, the Rust binary's dependency tree (`cargo cyclonedx`);
  - `opentrack-ui.cdx.json`, the UI bundle's (`npm sbom --sbom-format cyclonedx`).

## Making SBOMs locally

```sh
cargo install --locked cargo-cyclonedx cargo-deny
(cd ui && npm ci)                  # npm 10 or later (Node 22)
scripts/security/sbom.py           # target/sbom/opentrack.cdx.json, opentrack-ui.cdx.json
cargo deny check
```

`cargo cyclonedx` reads `cargo metadata`, which merges the features of every
target. That makes it list crates the Linux build never compiles, such as
`ring` through a WebAssembly-only edge of `quinn`. The script keeps only the
crates `cargo tree` reports as compiled into the binary. It records the ones
it dropped in the SBOM's metadata (`opentrack:filtered`), and it fails if a
banned crate is ever built.

## Releases

Publishing a GitHub release runs `.github/workflows/release.yml`, which:

1. Builds the image with BuildKit's SLSA provenance (`mode=max`) and SBOM attestations attached.
2. Pushes it to the package `ghcr.io/kineticlogic-io/opentrack:<tag>`. Releases up to
   v0.4.7 were made while the project was private and have been withdrawn.
3. Signs its digest with `scripts/release/sign-image`: cosign makes the payload, the **OpenSSL
   3.0.9 FIPS provider** (CMVP #4282, built from the Dockerfile's `openssl-fips` stage) signs it
   with the project key (ECDSA P-256, SHA-256), and cosign attaches the signature. cosign's own
   cryptography never touches the key (SC-13). The signature is not uploaded to a public transparency log. The script then verifies
   the signature.
4. Attaches the two SBOMs and `image-digest.txt` to the release.

**Verify an image:**

```sh
cosign verify --key cosign.pub --insecure-ignore-tlog=true ghcr.io/kineticlogic-io/opentrack@<digest>
docker buildx imagetools inspect ghcr.io/kineticlogic-io/opentrack@<digest> --format '{{ json .Provenance }}'
```

**The key:**
- The public key is `cosign.pub` in the repository. It was made on 2026-10-06 for the first
  open-source release and verifies every release from 1.0.0 on. The keys that signed the private-era
  images (0.4.x and earlier) were retired along with those images.
- The private key was made inside the FIPS provider (`scripts/release/new-signing-key`) and is
  kept as encrypted PKCS#8 (AES-256-CBC, PBKDF2-HMAC-SHA-256). It and its password are the
  repository secrets `RELEASE_SIGNING_KEY` and `RELEASE_SIGNING_PASSWORD`; the maintainer keeps a
  backup readable only by them, outside the repository.
- **Rotate it:** `scripts/release/new-signing-key <key.pem> <password-file> cosign.pub`, replace
  both secrets, keep the old public key beside it for the releases it signed, and re-sign the
  images still supported with `scripts/release/sign-image`.

The operating-system packages in the image come from Debian 12 (bookworm): the distroless base's
own, and the libraries `runtime-libs` copies in. Rebuild to pick up Debian security updates: the
`runtime-libs` stage installs the current packages, and moving the distroless digest updates the
rest. Scan each release image with
the scanner your accreditation uses (for example `grype` or `trivy`) and keep the report with the
release.

The Rust toolchain is pinned in `rust-toolchain.toml` (and in CI), so every build of a commit uses
the same compiler. Move it deliberately and run the tests when you do.
