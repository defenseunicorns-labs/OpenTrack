# OpenTrack on UDS Core Lite

The `bundle/uds-bundle.yaml` definition combines the OpenTrack Zarf package with UDS Data Valkey, PostgreSQL, and NATS. The chart creates the UDS Package resource for `opentrack.uds.dev`, including a Keycloak SAML client. Set `domain` in `chart/values.yaml` when using a different UDS domain.

## Build and deploy

From the repository root, with a running UDS Core cluster and access to the UDS Data package registry:

```sh
docker build --platform linux/arm64 -t localhost:5000/opentrack:uds-dev-0.1.1 .
uds zarf package create . --architecture arm64 --output build --confirm
uds create bundle --architecture arm64 --output build --confirm
uds deploy build/uds-bundle-opentrack-data-arm64-0.1.1.tar.zst --confirm
```

For an already deployed UDS Data bundle, deploy only the OpenTrack package using `uds zarf package deploy build/zarf-package-opentrack-arm64-0.1.1.tar.zst --confirm`. The UDS Data bundle must create the `opentrack` PostgreSQL credentials, copy the Valkey password into the `opentrack` namespace, and provide the NATS CA ConfigMap in the `nats` namespace. The chart resolves the Valkey password and NATS CA from those resources at install time.

To bootstrap a local administrator, create the `opentrack-bootstrap-admin` Secret in the `opentrack` namespace with `email` and `password` keys before installing the chart. The existing account is kept on subsequent deployments. SSO users authenticate through the UDS Keycloak SAML client; the chart maps the `/UDS Core/Admin` group to OpenTrack Admin and other SSO users to Viewer.

## Verify

```sh
kubectl -n opentrack get pods,packages,pvc
curl -f https://opentrack.uds.dev/healthz
```

The app must start with a NATS TLS-first connection and a successful Valkey connection. Check the pod logs for connection errors, then test sign-in from `https://opentrack.uds.dev/` with a UDS account.

## Current storage limitation

OpenTrack still uses SQLite for its control database. The chart provisions a persistent volume for that database, and it refuses `storage.backend: postgres` until the runtime backend is implemented. The UDS Data PostgreSQL database and credentials are provisioned for the planned backend; deploying this bundle does **not** make OpenTrack PostgreSQL-backed yet. Valkey is used for the live state and outbox, and NATS carries the track feed.
