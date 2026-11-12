# PostgreSQL control-store backend plan

OpenTrack currently uses SQLite for configuration and decisions, Redis for live
observations and tracks, and NATS JetStream for its published track picture.
The PostgreSQL backend will replace SQLite only when selected. Redis and NATS
retain their existing roles.

## Scope

- Keep SQLite as the default backend for existing installations.
- Add a PostgreSQL connection setting for new deployments. Selecting it must
  not open or create a SQLite database.
- Target fresh PostgreSQL databases in the first implementation. Importing an
  existing SQLite database is a separate migration task.
- Keep the current REST API, track output contract, audit chain, UID behavior,
  and configuration export format.

## Work required

1. Introduce a control-store interface used by the server's roles and CLI.
   The interface must cover configuration, users and sessions, sources,
   registry, correlation decisions, groups, the track graph, sync, audit,
   backup/export, and UID allocation. A backend selection belongs in `Common`
   and should be resolved before a role opens its store.
2. Build a PostgreSQL schema for the current logical data model, with its own
   version table and transactional migrations. Existing SQLite migrations
   remain append-only and continue to serve SQLite deployments. PostgreSQL
   constraints, JSON operations, generated IDs, and audit protections need
   explicit PostgreSQL equivalents.
   The PostgreSQL client must use OpenTrack's FIPS-approved cryptography for
   password authentication and TLS. The standard Rust `postgres` driver adds
   separate SHA-2 and MD5 implementations, so it is not suitable without a
   reviewed crypto integration. A libpq-based client linked to the runtime's
   OpenSSL FIPS provider is a candidate and needs build and runtime checks.
3. Implement the same store operations against PostgreSQL. Use transactions
   for each decision and its graph/audit changes. Allocate UIDs atomically;
   concurrent OpenTrack processes must never issue the same UID. The engine
   remains single-owner per Redis namespace, as it is today.
4. Make status, metrics, backup/restore guidance, session and sync key storage,
   and first-admin setup backend-aware. A PostgreSQL deployment should not
   require a writable SQLite volume. Any non-database files still need a
   persistent data directory or an explicit secret. `OT_DATA_DIR` now separates
   the session key, site password list, and plugin files from the SQLite path;
   when unset, it retains the existing SQLite-directory behavior.
5. Test the full API and engine flows with both backends. PostgreSQL tests
   need a real server and should cover concurrent UID allocation, decision
   undo, audit integrity, source revision history, and restart recovery.

## UDS integration

UDS Data's PostgreSQL Operator creates the `pg-cluster` database server in the
`postgres` namespace. The bundle should provision an OpenTrack database and
owner through that package, place credentials in the OpenTrack namespace, and
configure the PostgreSQL backend from a Secret. UDS Data's Valkey remains the
live Redis-compatible store; its NATS package provides the JetStream output.

The UDS package and bundle are scaffolded alongside this backend change.
They currently deploy the SQLite runtime with a persistent volume while
provisioning the PostgreSQL database for later use. The Helm chart rejects
`storage.backend: postgres` until the backend passes PostgreSQL integration
tests, so a deployment cannot claim PostgreSQL storage while opening SQLite.

## Schema verification

The `postgres-schema` CI job applies `crates/ot-store/postgres/0001_init.sql`
to a fresh PostgreSQL 16 database as its owner, then runs
`crates/ot-store/postgres/verify_schema.sql`. The verification checks JSON
validation, generated sync IDs, case-insensitive email uniqueness, audit
immutability (including `TRUNCATE`), and the authorized audit purge path.
It rolls back its fixtures. This verifies the schema foundation; the runtime
backend and end-to-end OpenTrack flows still require the work listed above.
