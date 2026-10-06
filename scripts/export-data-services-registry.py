#!/usr/bin/env python3
"""Export data-services' Redis registry (reg:*) as an OpenTrack registry import.

Migration tooling only: OpenTrack itself knows nothing about data-services.
Run it wherever the `redis` package and the Redis instance are reachable, e.g.

    docker exec -i ds-aisstream python - < scripts/export-data-services-registry.py > registry.json
    curl -X POST -H 'content-type: application/json' --data @registry.json \
         http://127.0.0.1:8090/api/v1/registry/import

Values are kept as stored (strings); `name` and `status` are promoted to the
entity, managed timestamps are dropped, and everything else goes to `fields`.
Identifier-like fields (currently `imo`) become typed identifiers, so every
identifier an entity has is stored as scheme:value (mmsi:..., imo:...).
"""

import json
import os
import sys

import redis

r = redis.Redis(host=os.environ.get("REDIS_HOST", "127.0.0.1"), port=int(os.environ.get("REDIS_PORT", "6379")),
                decode_responses=True)
MANAGED = {"status", "created", "updated", "name", "source"}
#: Entity fields that are really identifiers: field -> scheme.
IDENTIFIER_FIELDS = {"imo": "imo"}
PLACEHOLDERS = {"0", "1", "1234567"}

entities = []
for eid in sorted(r.smembers("reg:entities")):
    h = r.hgetall(f"reg:entity:{eid}")
    if not h:
        continue
    identifiers = []
    for ident in sorted(r.smembers(f"reg:entity:{eid}:ids")):
        scheme, _, value = ident.partition(":")
        i = r.hgetall(f"reg:id:{scheme}:{value}")
        if i.get("entity") != eid:
            continue  # dangling or reassigned
        identifiers.append({k: v for k, v in {
            "scheme": scheme, "value": value,
            "expected_name": i.get("expected_name") or None, "source": i.get("source") or None,
        }.items() if v is not None})
    for field, scheme in IDENTIFIER_FIELDS.items():
        value = (h.get(field) or "").strip()
        if value and value not in PLACEHOLDERS and not any(
                i["scheme"] == scheme and i["value"] == value for i in identifiers):
            identifiers.append({"scheme": scheme, "value": value, "source": h.get("source") or "registry field"})
    entities.append({
        "id": eid,
        "name": h.get("name") or None,
        "status": h.get("status", "active"),
        "source": h.get("source") or None,
        "fields": {k: v for k, v in h.items() if k not in MANAGED and k not in IDENTIFIER_FIELDS and v != ""},
        "identifiers": identifiers,
    })

json.dump({"label": "data-services reg:* export", "entities": entities}, sys.stdout)
print(f"exported {len(entities)} entities", file=sys.stderr)
