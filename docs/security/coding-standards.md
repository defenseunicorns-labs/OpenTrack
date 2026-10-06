# Coding standards

The rules OpenTrack's code is written to. Most are enforced by a tool, in CI
(`.github/workflows/ci.yml`) and in `scripts/check.sh`, which runs the same checks locally. The
rest are checked in review. A change that breaks one is not merged.

## Enforced by tools

| Rule | Tool | Where |
|---|---|---|
| Rust is formatted by rustfmt, with its defaults | `cargo fmt --all --check` | CI `rust` job, `scripts/check.sh` |
| No clippy warnings, in every target (tests included) | `cargo clippy --all-targets --locked -- -D warnings` | CI `rust` job, `scripts/check.sh` |
| No `unsafe` code in OpenTrack's crates | `unsafe_code = "forbid"` in `[workspace.lints.rust]` (`Cargo.toml`); all nine crates take the workspace lints | The compiler |
| No `dbg!` left in | `dbg_macro = "warn"`, which `-D warnings` makes an error | clippy |
| The tests pass, including those against real Redis, NATS and MQTT | `cargo test --locked` with `OT_TEST_REDIS_URL`, `OT_TEST_NATS_URL`, `OT_TEST_MQTT_URL` | CI `rust` job, `scripts/check.sh` |
| Builds use the locked dependency versions | `--locked` (Cargo), `npm ci` (UI) | CI, `Dockerfile` |
| One compiler for every build of a commit | `rust-toolchain.toml` pins 1.98.1; CI names the same version | rustup, CI |
| Dependencies meet the supply-chain policy (below) | `cargo deny check` against `deny.toml`; `npm audit --omit=dev --audit-level=high` | CI `supply-chain` job |
| The UI type-checks | `tsc -b` (part of `npm run build`); TypeScript 6 is strict by default and `tsconfig.app.json` does not turn it off; unused locals and parameters are errors | CI `ui` job |
| The UI lints | `oxlint` with the React, TypeScript and oxc plugins (`ui/.oxlintrc.json`); `react/rules-of-hooks` is an error | CI `ui` job |
| The UI tests pass | `vitest run` | CI `ui` job |

`scripts/coverage.sh` measures test coverage; see [scm-plan.md](scm-plan.md#releases).

## Cryptography (FIPS)

All cryptography goes through the FIPS 140-3 validated modules ([fips.md](fips.md)):
- In Rust, through `aws-lc-rs` with its `fips` feature, or rustls with the AWS-LC FIPS provider
  that `fips::init` installs at start (`crates/ot-server/src/fips.rs`). Random values, hashes,
  HMACs and password hashing use the helpers in `fips.rs`.
- No other crypto crate for a security purpose. `ring`, `openssl-src` and `native-tls` are banned
  in `deny.toml`, so CI fails if one enters the build.
- A dependency that brings TLS must be built without its `ring` default (see how `async-nats`,
  `rustls`, `tokio-rustls`, `reqwest`, `redis` and `rcgen` are declared in `Cargo.toml`). If it
  cannot be, patch it as `third_party/rumqttc` is, with a note.
- A change to `aws-lc-fips-sys` in `Cargo.lock` is a change of cryptographic module: say so in the
  pull request and the CHANGELOG.

## Secrets

- A source's secrets (passwords, tokens, header and metadata values, credentials in URLs, secret
  named fields in messages) are masked for everyone but admins by `ot_core::secrets`
  (`redact_spec`, `redact_url`; re-exported as `ot_source::secrets`). New transport fields that
  hold a secret must be covered by its rules, with a test.
- Secrets in source specs are written as `${env:NAME}` where the deployment allows; the code
  resolves them when the source starts, never when it is saved.
- Never log a secret, and never put one in the decision log or the audit record: source changes
  are recorded masked.
- Key files the server writes (`session.key`, `initial-admin.txt`) are created readable by their
  owner only (mode 0600).

## Errors

- A handler returns `ApiError` (`crates/ot-server/src/control.rs`). A server-side failure is
  `ApiError::internal`: the caller gets `internal error (reference …)` and the reference; the
  detail (SQLite, Redis, file paths) goes only to the log with that reference.
- Client errors (400, 404, 409, 422) say what was wrong with the request, in words the caller can
  act on, and nothing about the server's internals.
- No `unwrap` or `expect` on input from outside the process. They are acceptable in tests and on
  values the code has just checked.
- Status details (paths, URLs, errors) go to admins only.

## Input

- Everything from outside is decoded into typed values (serde) before it is used; unknown fields
  are refused where a typo would silently change behaviour (`deny_unknown_fields` on source
  specs, transports, pipelines, mappings and settings).
- Every input has a size limit: request bodies (axum's default, and explicit limits for
  configuration import, plugins and registry sheets), frames (4 MiB by default), gRPC messages
  (`max_message_kib`), SAML responses (256 KB, refused before parsing, no DTD).
- File paths from users are confined to the data directory (file sources, probes).
- URLs an admin gives are checked for scheme and form before they are used (basemap tiles:
  `http`/`https`, no credentials, no redirects followed).
- Every `/api/v1` route's role comes from `crates/ot-server/src/auth/policy.rs`. A route it does
  not name is readable by viewers and changeable only by admins; a new route gets a line in its
  test (`each_path_needs_its_role`).

## User interface

- No `dangerouslySetInnerHTML`, `innerHTML`, `eval` or `new Function`. There are none today; the
  content security policy (`control.rs`, `CSP`) also refuses inline and evaluated script.
- Markdown (the in-app help) is rendered by `react-markdown` without raw HTML.
- The browser never holds a secret it does not need: the session is an `HttpOnly` cookie, tile
  server URLs stay on the server.
- Components come from stareSDK where it has them; a missing component is raised before one is
  built.

## Dependencies

- A new dependency needs a reason in the pull request, a licence on the allow list in `deny.toml`,
  and must come from crates.io (or npm) unless it is vendored in `third_party/` with an
  `OPENTRACK.md` saying what differs from upstream and when to drop the copy.
- Versions are pinned by the lockfiles (`Cargo.lock`, `ui/package-lock.json`); change them on
  purpose, in their own commit or pull request, and run the tests.
- An advisory that cannot be fixed at once is ignored in `deny.toml` only with a reason, and the
  exception is reviewed at each release.
- Prefer a dependency's minimal feature set (`default-features = false`).

## Tests and review

- A change comes with tests: a fix with a test that failed before it, a feature with tests of what
  it promises. Security behaviour (roles, masking, refusals, limits) is always tested.
- Documentation changes in the same pull request as the behaviour: the guides, and for security
  controls `stig-mapping.md` and the accreditation sources (`docs/security/ato/`).
- The reviewer checks the rules in this document that tools cannot: secrets, errors, input
  limits, roles on new routes, cryptography, dependencies, tests and documentation. How review
  and merging work, and their current limits, is in [scm-plan.md](scm-plan.md#change-control).
