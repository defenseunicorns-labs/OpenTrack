# OpenTrack on UDS Core Lite

The `bundle/uds-bundle.yaml` definition combines the OpenTrack Zarf package with UDS Data Valkey, PostgreSQL, and NATS. The chart creates the UDS Package resource for `opentrack.uds.dev`, including a Keycloak SAML client. Set `domain` in `chart/values.yaml` when using a different UDS domain.

## Build and deploy

From the repository root, with a running UDS Core cluster and access to the UDS Data package registry:

```sh
docker build --platform linux/arm64 -t localhost:5000/opentrack:uds-dev-0.1.2 .
uds zarf package create . --architecture arm64 --output build --confirm
uds create bundle --architecture arm64 --output build --confirm
uds deploy build/uds-bundle-opentrack-data-arm64-0.1.3.tar.zst --confirm
```

For an already deployed UDS Data bundle, deploy only the OpenTrack package using `uds zarf package deploy build/zarf-package-opentrack-arm64-0.1.3.tar.zst --confirm`. The UDS Data bundle must create the `opentrack` PostgreSQL credentials, copy the Valkey password into the `opentrack` namespace, and provide the NATS CA ConfigMap in the `nats` namespace. The chart resolves the Valkey password and NATS CA from those resources at install time.

To bootstrap a local administrator, create the `opentrack-bootstrap-admin` Secret in the `opentrack` namespace with `email` and `password` keys before installing the chart. The existing account is kept on subsequent deployments. SSO users authenticate through the UDS Keycloak SAML client; the chart maps the `sso.adminGroup` group (default `/C-UAS/OpenTrack Admin`) to OpenTrack Admin and other SSO users to Viewer. Create this group in the UDS Keycloak realm and assign the intended administrators to it. OpenTrack fetches metadata over the internal Keycloak service, then rewrites its URLs to the browser-facing `https://sso.<domain>` origin. Set `sso.publicOrigin` if the Keycloak browser URL differs.

OpenTrack currently sends unsigned SAML sign-in requests. The chart sets `saml.client.signature: "false"` for its Keycloak client so Keycloak accepts them. OpenTrack still verifies signed SAML responses. Signing the outgoing requests is future work before enabling Keycloak's client-signature requirement.

## Verify

```sh
kubectl -n opentrack get pods,packages,pvc
curl -f https://opentrack.uds.dev/healthz
```

The app must start with a NATS TLS-first connection and a successful Valkey connection. Check the pod logs for connection errors, then test sign-in from `https://opentrack.uds.dev/` with a UDS account.

## Storage

The chart defaults to `storage.backend: sqlite` for standalone deployments. The C-UAS UDS bundle overrides it to `postgres`, using the `opentrack` database and credentials created by the UDS Data PostgresOperator. The PostgreSQL password is read from a mounted Secret file. The `/data` volume holds session and sync signing keys in PostgreSQL mode. Valkey stores live state and the outbox; NATS JetStream carries the track feed. PostgreSQL mode targets fresh databases; importing an existing SQLite database is not yet supported.
