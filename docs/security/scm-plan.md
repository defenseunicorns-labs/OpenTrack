# Software configuration management plan

How OpenTrack's source, builds and releases are controlled: what is under configuration
management, how a change gets from an idea to a release, how releases are identified, signed and
recorded, and who does what. It covers the product. A site's configuration management of its own
deployment is the site's plan (CM-9); [Deployed nodes](#deployed-nodes) says what OpenTrack gives
it.

OpenTrack is developed by a small team: today one maintainer. Where this plan names a practice
the tools do not enforce, it says so.

## Configuration items

Everything below is in the Git repository `kineticlogic-io/OpenTrack` (public, on GitHub) and changes
only through [change control](#change-control), except the released artefacts, which are built
from a tagged commit.

| Item | Where | How it is controlled |
|---|---|---|
| Rust source | `crates/*` (nine crates, one workspace), `wit/plugin.wit` | Pull requests; `cargo fmt`, clippy, tests ([coding-standards.md](coding-standards.md)) |
| UI source | `ui/src`, `ui/index.html`, `ui/public` | Pull requests; type check, oxlint, tests |
| Plugin SDKs and examples | `sdk/rust`, `sdk/python` | Pull requests; their own workspace |
| Tracker profiles shipped | `profiles/trackers` | Pull requests |
| Rust dependency versions | `Cargo.lock`, built with `--locked` | Changed deliberately; checked by cargo-deny against `deny.toml` |
| UI dependency versions | `ui/package-lock.json`, installed with `npm ci` | Changed deliberately; `npm audit` |
| Compiler | `rust-toolchain.toml` (Rust 1.98.1, clippy, rustfmt, `wasm32-wasip2`); CI names the same version | Moved deliberately, with the tests ([supply-chain.md](supply-chain.md)) |
| Base images | `Dockerfile`: `node:22-bookworm-slim`, `debian:bookworm-slim`, `rust:1-bookworm`, each pinned by digest | Moved as [supply-chain.md](supply-chain.md) says, noted in CHANGELOG |
| Build inputs fetched in the image | `Dockerfile`: Go 1.27.1 and the OpenSSL 3.0.9 source, each checked by SHA-256 | As base images |
| OpenSSL FIPS configuration | `docker/openssl-fips.cnf` | Pull requests; a crypto change ([fips.md](fips.md)) |
| Forked crates | `third_party/samael` (SAML), `third_party/rumqttc` (MQTT without `ring`), each with an `OPENTRACK.md` saying what differs from upstream and when to drop the fork | Pull requests; reviewed at each release against upstream |
| Supply-chain policy | `deny.toml` | Pull requests; every advisory exception has a reason |
| CI and release workflows | `.github/workflows/ci.yml`, `release.yml`; actions pinned by commit SHA | Pull requests |
| Signing | `cosign.pub`, `scripts/release/`; the private key and password are repository secrets, with the maintainer's offline backup | [supply-chain.md](supply-chain.md) |
| Branch protection | `scripts/github/protect-main.sh` | Re-run after any change to it |
| Deployment samples | `docker-compose.yml`, `docs/examples/sapient/docker-compose.yml`, `docs/examples/` | Pull requests |
| Documentation | `README.md`, `CHANGELOG.md`, `SECURITY.md`, `docs/` (guides, security documents) | Pull requests, in the same pull request as the behaviour they describe |
| Accreditation package sources | `docs/security/ato/evaluations/*.json`, `ssp/controls.json`, `poam.json`; the generated files are made by `scripts/ato/*`, never edited | Pull requests; regenerated for each release |
| Scripts | `scripts/` (checks, coverage, SBOMs, scans, benchmark) | Pull requests |

**Released artefacts**, per version, identified by the tag `v<version>`:

| Artefact | Where |
|---|---|
| Container image | `ghcr.io/kineticlogic-io/opentrack:v<version>`, identified by its digest, signed, with SLSA provenance and SBOM attestations |
| `image-digest.txt`, `opentrack.cdx.json`, `opentrack-ui.cdx.json` | Attached to the GitHub release |
| Release notes | The GitHub release: the version's CHANGELOG section |
| Vulnerability scan | `docs/security/ato/scans/v<version>/` (`scan.md`, `trivy.json`, `grype.json`) |
| Coverage summary | Attached to the GitHub release ([Releases](#releases)) |

Build outputs (`target/`, `ui/dist`, `ui/node_modules`, coverage reports) are never committed.

## Repository and branches

- **`main`** is the only long-lived branch. Every release is tagged on it.
- **Work happens on short-lived branches** named for the change (`security-fixes`,
  `cot-output`, `release-0.4.3`), merged into `main` by pull request and then deleted.
- **Branch protection** on `main` (`scripts/github/protect-main.sh`): changes land through pull
  requests with one approval from someone other than the author, stale approvals are dismissed on
  new commits, CI's `rust`, `supply-chain` and `ui` jobs must pass on a branch up to date with
  `main`, and force-pushes and deletion are refused.
- **Admins may bypass it** (`enforce_admins: false`). With one maintainer there is no second
  person to approve, so today the maintainer merges their own pull requests with the admin
  bypass. The pull request and its description are still the record of the change.
  When a second maintainer joins, approvals become real and the bypass is kept for emergencies.
- **While GitHub Actions minutes are exhausted** (since 29 September 2026, when GitHub stopped
  starting jobs for billing), CI does not run. Before merging, the maintainer runs
  `scripts/check.sh` on the test server (the same format, clippy, test and UI checks as CI, with
  Redis, NATS and MQTT test containers) and `cargo deny check`, and merges with the admin bypass.
  Those commits carry `[skip ci]` in their subject, so no run is queued. When minutes are
  available again, CI runs again on its own and the required checks gate merges as before.

## Versions and the CHANGELOG

- **Version numbers** are `MAJOR.MINOR.PATCH`. OpenTrack is alpha (0.x): a minor version (0.4.0,
  0.5.0) marks a milestone and may change the published message, the plugin interface or the
  database schema; a patch version (0.4.3) adds fixes and smaller features within it. The
  version is set in one place for the Rust crates (`[workspace.package]` in `Cargo.toml`) and in
  `ui/package.json`, and both change in the release commit.
- **Tags** are `v<version>` on the release's merge commit on `main`.
- **`CHANGELOG.md`** has an `## Unreleased` section that every change adds to. At release it
  becomes `## <version> (alpha), <date>`, with **Upgrade notes** (what a site must do),
  **Security fixes** when there are any, then the changes by area, citing issue numbers.
- **The running version** is shown in Overview → System status and `GET /api/v1/status`, and
  exported as the OpenTelemetry `service.version`.

## Releases

A release is prepared on a `release-<version>` branch and published from `main`:

1. **Prepare.** Bump the version in `Cargo.toml` and `ui/package.json`; turn `## Unreleased`
   into the release's CHANGELOG section; update the version the guides and security documents
   name ("Written for OpenTrack …"). Re-evaluate the accreditation rules the release touches
   (`docs/security/ato/evaluations/`, `ssp/controls.json`, `poam.json`) and regenerate the
   package (`scripts/ato/checklist`, `scripts/ato/ssp`, `scripts/ato/poam`).
2. **Check.** `scripts/check.sh` and `cargo deny check` pass (in CI, or on the test server while
   CI is unavailable). Run `scripts/coverage.sh` and keep `target/coverage/summary.txt`.
3. **Merge** the release pull request and **tag** the merge commit `v<version>`.
4. **Publish a GitHub pre-release** for the tag, titled `<version> — <summary>`, with the
   CHANGELOG section as its notes. Publishing runs `.github/workflows/release.yml`, which:
   builds the image with BuildKit's SLSA provenance (`mode=max`) and SBOM attestations, pushes it
   to `ghcr.io/kineticlogic-io/opentrack:v<version>`, signs its digest with the project's cosign key
   (no public transparency log), verifies the signature, and attaches
   the SBOMs (`scripts/security/sbom.py`) and `image-digest.txt` to the release.
   **While GitHub Actions is unavailable**, the maintainer does the same on the test server: build
   and push with `docker buildx build --provenance=mode=max --sbom=true`, `scripts/release/sign-image` (the
   OpenSSL FIPS provider signs the cosign payload with the project key), `cosign verify --key cosign.pub
   --insecure-ignore-tlog=true`, then `scripts/security/sbom.py` and `gh release upload` for the
   SBOMs and `image-digest.txt`. v0.4.2 and v0.4.3 were released this way.
5. **Scan** the pushed image: `scripts/ato/scan ghcr.io/kineticlogic-io/opentrack:v<version>`. Commit
   the scan to `docs/security/ato/scans/v<version>/` and carry its Critical and High findings
   into the POA&M, in a follow-up pull request.
6. **Attach the coverage summary** (`target/coverage/summary.txt`, renamed
   `coverage-summary.txt`) to the GitHub release with `gh release upload`.

**Test coverage** is measured by `scripts/coverage.sh`: `cargo llvm-cov` over the whole
workspace, with the tests against real Redis, NATS and MQTT. The UI's tests run without a
coverage figure: `vitest --coverage` needs a coverage provider (`@vitest/coverage-v8`), which is
not a UI dependency; the script measures the UI too wherever one is installed. Coverage is
recorded, not gated: there is no minimum a change must meet, and a release that loses coverage
says why in its pull request.

Coverage when this practice started (the Rust workspace at 0.4.3, 2026-09-30; the run takes about
10 minutes):

| Measure | Covered |
|---|---|
| Lines | 83.2% of 45,874 |
| Functions | 80.6% of 5,424 |
| Regions | 81.8% of 82,904 |

## Change control

- **Changes start from an issue** as a rule; small fixes that explain themselves can go straight
  to a pull request. Issues are labelled by kind (`bug`, `enhancement`, `roadmap`, `security`,
  `documentation`) and area (`auth`, `sources`, `correlation`, `ui`, `ops`, `ci`). The roadmap
  is the pinned issue #28, which links the milestone issues.
- **Decisions are recorded in issues.** An open product decision is an issue labelled
  `roadmap` ("Planned work and open product decisions"); the decision, and why, is written in the
  issue before it is closed. Accepted risks (such as F-4) are also in `stig-mapping.md`.
- **A pull request** names its issue (`#57`), says what changed and why, lists what was checked,
  and carries its tests and documentation changes. It is reviewed against
  [coding-standards.md](coding-standards.md). A change to a security-critical component
  ([threat model, criticality analysis](threat-model.md#criticality-analysis)) says so.
- **Security fixes** follow [SECURITY.md](../../SECURITY.md): tracked privately until released,
  then an issue labelled `security`, a **Security fixes** CHANGELOG entry and release notes.
- **Records.** Git history, pull requests and issues are the change record; CHANGELOG and the
  release notes are the release record; the accreditation package is regenerated from reviewed
  sources for each release.

## Deployed nodes

What a site controls, and the tools OpenTrack gives it:

| Configuration | Where it lives | How it is controlled |
|---|---|---|
| Deployment settings (`OT_*`, `OTEL_*`) | The environment, `.env` beside `docker-compose.yml` | The site's CM; listed in the [admin guide, Configuration](../guides/admin.md#configuration) |
| Runtime configuration: sources, output schema, correlation, sign-in, accounts, plugins, TAK outputs | SQLite, `/data/opentrack.db` | Changed by admins in the UI or API; each change is a decision (who, when, what) in the decision log and the audit record; sources and the schema keep every revision |
| The whole runtime configuration as one file | `opentrack config export` / `GET /api/v1/export/config` | Kept as a baseline, compared between exports, and used to rebuild a node (`opentrack config import`); it holds secrets ([admin guide](../guides/admin.md#configuration-export)) |
| The image | A signed digest | Verified before running ([hardening.md](hardening.md)); upgrades as the [admin guide](../guides/admin.md#upgrades) says |

The account policy, lockout, session limits and audit retention are fixed at the STIG values and
are not configuration.

## Roles

| Role | Who | Responsible for |
|---|---|---|
| Maintainer | Parker Hornstein (@phornstein) | Product decisions, the roadmap, reviewing and merging pull requests, releases, signing keys, repository settings, security reports and advisories. Today also the only developer and reviewer |
| Contributor | Anyone given access to the repository | Branches and pull requests following this plan and [coding-standards.md](coding-standards.md) |
| Reviewer | A second maintainer, when there is one | Approving pull requests; until then the maintainer's own review, recorded in the pull request |
| Site administrator, ISSO | The adopting site | The deployment's configuration, the site's CM plan, verifying images, forwarding vulnerabilities ([SECURITY.md](../../SECURITY.md)) |

This plan is reviewed with each minor release, alongside the [threat model](threat-model.md).
