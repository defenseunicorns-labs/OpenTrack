# Worked examples

Complete source configurations, each entered through the API exactly as an admin would. Nothing about
any feed is built into OpenTrack; each is reproduced from these files alone.

| Example | Transport | Codec | What it teaches |
|---------|-----------|-------|-----------------|
| [`aisstream.json`](aisstream.json) | WebSocket client with a subscribe message | JSON | Subscribe-on-connect with a secret from the environment, record rejection (MMSI rules), a per-message-type body (`let` + dynamic `key`), the static join (AIS types 5, 19 and 24 onto position reports), registry lookup and grading (any identifier scheme; here MMSI), country-based affiliation, a military-only filter, throttling |
| [`adsb-lol.json`](adsb-lol.json) | HTTP poll every 5 s | JSON with a record path (`ac`) and frame context (`now`) | Polling, splitting one response into many records, unit transforms (knots, feet, ft/min), time arithmetic, lookup tables, ICAO address blocks to countries (`ranges`), tiered values (`cases`) |
| [`stanag4607.json`](stanag4607.json) | TCP client, framed by a length field in the packet header | STANAG 4607 codec plugin | Real GMTI: every dwell target a detection with a ground error ellipse from the radar geometry, a GNN tracker timed by the revisit rate it measures (`auto_timing`) with an existence model, the sensor platform as its own friendly track (a `track` rule that bypasses the tracker, symbol from the platform type). Stream recordings with `scripts/benchmark/replay/gmti-rebroadcast.py` |
| [`gps-udp.json`](gps-udp.json) | UDP | JSON | A GPS track feed (live training), as `scripts/benchmark/replay/gmti-gps-replay.py` sends an exercise's GPS logs alongside its GMTI |
| [`autoferry/`](autoferry) | UDP | JSON | The Autoferry demo: a vessel track feed and lidar and radar detections through GNN and MHT trackers (`scripts/benchmark/replay/autoferry-replay.py`) |
| [`sapient-acoustic-mqtt.json`](sapient-acoustic-mqtt.json) | MQTT subscriber | SAPIENT BSI Flex 335 v2.0 protobuf | One fixed acoustic array; `scripts/demo-sapient-acoustic.sh` creates all five sources and maps their ranged detections, with bearings-only reports left available for cross-fixing |

## Using them

Both mappings target extension schema version 2, defined in [`schema.json`](schema.json). Publish it first
(version 1 is the core schema with no extensions):

```sh
curl -X PUT -H 'content-type: application/json' --data @docs/examples/schema.json \
     http://127.0.0.1:8090/api/v1/schema/draft
curl -X POST http://127.0.0.1:8090/api/v1/schema/draft/publish
```

Then add the sources:

```sh
# aisstream needs its API key in the server's environment; the spec only references it.
export AISSTREAM_API_KEY=...

curl -X POST -H 'content-type: application/json' --data @docs/examples/adsb-lol.json \
     http://127.0.0.1:8090/api/v1/sources
curl -X POST http://127.0.0.1:8090/api/v1/sources/adsb-lol/enable
curl http://127.0.0.1:8090/api/v1/sources/adsb-lol          # config + live status
curl http://127.0.0.1:8090/api/v1/sources/adsb-lol/metrics  # per-minute counters
```

Dry-run a spec against sample frames before saving it (nothing is stored or published):

```sh
curl -X POST -H 'content-type: application/json' \
     --data '{"spec": <spec>, "samples": ["<frame 1>", "<frame 2>"]}' \
     http://127.0.0.1:8090/api/v1/sources/validate
```

Add `"trace": 5` (at most 10) to also get `trace`: the first 5 decoded records (samples), in
decode order, stage by stage, as the pipeline designer's live preview shows them:
`{"frames": [{"format": "json" | "text" | "binary", "bytes": n, "content": …}], "samples":
[{"frame": <index in frames>, "stages": [{"id": "decode", "items": [...], "dropped":
["<reason>"], "held": true}, ..., {"id": "publish", "items": [<message>]}]}]}`. Each frame the
samples came from is listed once; a frame that does not decode is a sample of its own. Only the
stages the pipeline has are listed, `dropped` and `held` appear only when set, and an item or frame
over 64 KiB is cut to a `truncated` marker. Without `trace` the response is unchanged.

## The pipeline, in order

1. **Transport** delivers frames (`websocket`, `http_poll`, `tcp_client`, `tcp_server`, `udp`, `mqtt`), with framing
   for stream transports (`lines`, `length_prefix`, `delimiter`, `end_tag`). String settings may reference
   `${env:NAME}`; secrets are never stored. Transport metadata is added to every record under `_frame`; for MQTT
   that is the message's `topic`, its `topic_levels` (split on `/`) and `retained` for retained messages:

   ```json
   "transport": { "type": "mqtt", "url": "mqtts://broker:8883", "topics": ["ais/+/position"], "qos": 1,
                  "username": "opentrack", "password": "${env:MQTT_PASSWORD}" },
   ...
   "mapping": { "rules": [{ "name": "position", "key": "_frame.topic_levels[1]",
                            "identifiers": [{ "scheme": "mmsi", "value": "_frame.topic_levels[1]" }], ... }] }
   ```

   Client transports connect over TLS with a `tls` object (a private CA, a client certificate for mutual TLS);
   `tcp_server` accepts TLS only with one, and with `client_ca_file` only clients holding a certificate that CA signed:

   ```json
   "transport": { "type": "tcp_client", "host": "feed.example", "port": 8089,
                  "tls": { "ca_file": "/etc/opentrack/feed-ca.pem", "cert_file": "/etc/opentrack/client.pem",
                           "key_file": "${env:OT_CLIENT_KEY}", "server_name": "feed.example" } }
   "transport": { "type": "tcp_server", "bind": "0.0.0.0:8089",
                  "tls": { "cert_file": "/etc/opentrack/server.pem", "key_file": "/etc/opentrack/server.key",
                           "client_ca_file": "/etc/opentrack/clients-ca.pem" } }
   ```

   MQTT's older top-level `ca_file` still works; use `tls.ca_file` alongside a client certificate. A key without
   a certificate (or the reverse), or TLS settings on a plain `http://`, `ws://` or `mqtt://` URL, fail validation.
2. **Codec** turns a frame into records (`json`, `cot_xml`, `xml`).
3. **Reject** rules drop records before mapping, counted per reason (`rejected:<reason>`).
4. **Mapping** rules map records to the track schema. Every matching rule applies; `static` rules feed the static
   join, `observation` rules yield track reports.
5. **Static join** fills identity fields missing from a report from the latest static record with the same key.
6. **Registry** resolves every identifier the track carries, of any scheme (`mmsi`, `icao`, `elnot`, hull numbers,
   anything an admin registers; `schemes` optionally restricts and prioritises them). It grades the match
   (`exact`, `hull`, `name`, `generic`, `stale`), applies entity fields at corroborated grades, and records
   `ext.registry`. Identifiers that resolve to different entities are recorded as a conflict and nothing is applied.
7. **Affiliation** maps a country code to friend / hostile / neutral / otherwise.
8. **Filter** keeps or drops reports (`keep_if`, `drop_if`).
9. **Throttle** limits writes per source track (minimum interval, heartbeat, minimum movement).

Value specs, conditions and transforms are documented in `crates/ot-source/src/expr.rs`; mapping targets in
`crates/ot-source/src/mapping.rs`.

## Where the tables came from

The lookup tables in these files (AIS ship type to CoT, aircraft designators and emitter categories, ICAO address
blocks, allied and adversary country lists, registry token lists) were exported from the data-services modules
they replace, so the examples reproduce that behaviour exactly. They are data: edit them here, not in code.

## Known differences from data-services

- Throttling uses each report's own timestamp; data-services used the receive clock (AIS) or snapshot time
  (adsb.lol). Throttled reports are not counted towards a track's observation count.
- data-services copied every raw feed field into `attributes_json`; the examples map a chosen set of extension
  fields (`ext.*`) instead.
- Track ids become `tms-<UID>` system ids, one per source track until correlation arrives (phase 3).

## Moving the registry

`scripts/export-data-services-registry.py` exports data-services' Redis registry (`reg:*`) in the import format:

```sh
docker exec -i ds-aisstream python - < scripts/export-data-services-registry.py > registry.json
curl -X POST -H 'content-type: application/json' --data @registry.json http://127.0.0.1:8090/api/v1/registry/import
```

An import never moves an identifier from one entity to another; such cases are reported as conflicts.
