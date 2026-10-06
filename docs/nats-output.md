# Consuming OpenTrack system tracks over NATS

OpenTrack publishes every system track to a NATS JetStream stream. This is the whole contract a
consumer (OpenStare) needs.

## Stream and subjects

| | Default | Setting |
|-|---------|---------|
| Stream | `TRACKS` | `OT_NATS_STREAM` |
| Subjects | `tracks.>` | `OT_NATS_TRACKS_SUBJECT` (the prefix) |
| One track | `tracks.tms-OTK000000123` | |
| A deleted history point | `tracks.history.tms-OTK000000123.<Unix ms>` | |
| Messages kept | 1 per subject (the latest) | |
| Max age | 24 h without a newer message | `OT_NATS_MAX_AGE_HOURS` |
| Duplicate window | 2 min | |

OpenTrack creates the stream with these settings if it does not exist, and never changes an
existing one, so an operator may create it beforehand (for example with different storage or
replicas). It must capture `tracks.>` and should keep one message per subject.

Because the stream keeps the latest message per track, a consumer that starts late gets the whole
current picture by reading the stream from the start (for example an ordered consumer with
`DeliverPolicy::LastPerSubject` on `tracks.>`), then keeps receiving updates. Live tracks are
republished whenever they change, at most every 5 s each (immediately when identity,
classification or the track's entity changes); a track with no report for 6 h is dropped and deleted.

## Headers

| Header | Value |
|--------|-------|
| `OT-Op` | `upsert`, `delete` or `delete_history_point` |
| `OT-Schema` | `opentrack.track.v2` |
| `Nats-Msg-Id` | unique per update, e.g. `opentrack-OTK:1790303694426-0` |

## `upsert`

Two parts:

* **The OTH-GOLD minimum**, always present. These are the fields OS-OTG Rev C makes mandatory
  in a contact report: the track number, class-name, force code and track type (CTC fields 1, 2,
  11 and 13), and the position with its time (POS fields 1-4). OpenTrack adds one more required
  field, the symbol identification code (`sidc`).
* **`attributes`**, defined by the administrator. It holds exactly the fields of the published
  output schema that have a value for this track, and nothing else. Which fields exist, their
  types and their notes are set in OpenTrack's Schema workspace, so the set changes when an admin
  publishes a new schema version; consumers should treat unknown keys as data, not errors.

An upsert replaces whatever the consumer holds for that `track_id`. Timestamps are RFC 3339 UTC,
units are SI (metres, metres per second, degrees true), enums are lower-case strings, and an
attribute without a value is omitted rather than sent as null.

```json
{
  "schema": "opentrack.track.v2",
  "op": "upsert",
  "track_id": "tms-OTK000000001",
  "uid": "OTK000000001",
  "class": "UNEQUATED",
  "name": "TED STEVENS",
  "domain": "surface",
  "affiliation": "unknown",
  "force_code": 30,
  "track_type": "tactical",
  "sidc": { "standard": "cot", "code": "a-u-S" },
  "time": "2026-09-25T03:21:14.990Z",
  "lat": 32.701,
  "lon": -117.2,
  "attributes": {
    "contact_phone": "+1 555 0100",
    "destination": "LONG BEACH",
    "state": "confirmed"
  },
  "publisher": { "node_id": "opentrack-OTK", "version": "0.3.1", "correlation": "correlation-5" },
  "published_at": "2026-09-25T03:21:17.000676087Z"
}
```

### The GOLD minimum

| Field | GOLD | |
|-------|------|-|
| `track_id` | CTC 1 | `tms-<UID>`: the key. Stable for the life of the track. `uid` is the bare UID. |
| `class` | CTC 2 | platform class in capitals, `UNEQUATED` when unknown |
| `name` | CTC 2 | platform name in capitals, `UNKNOWN` when unknown (GOLD writes these as `class-name`) |
| `domain` | CTC 11 | force code position: `air`, `surface`, `subsurface`, `ground`, `space` or `unknown` |
| `affiliation` | CTC 11 | force code threat identity: `pending`, `unknown`, `assumed_friend`, `friend`, `neutral`, `suspect`, `hostile`, `joker`, `faker` or `none` |
| `force_code` | CTC 11 | the GOLD force code (Table 5-1), e.g. 9 surface friend, 30 surface unknown |
| `track_type` | CTC 13 | `tactical`, `live_training`, `simulated_training` or `demand_entry` |
| `sidc` | | symbol identification code: `{ "standard", "code" }`, see below |
| `time` | POS 1-2 | when the published position was observed |
| `lat`, `lon` | POS 3-4 | degrees, WGS84 |

`security`, present only when a source reporting for the track is labelled, is the track's
security label in the shape OpenStare's ES index ICD reserves for its `stare-security` component
template: `{"classification": "SECRET", "restrictions": ["NOFORN"], "sharing": "REL TO USA, FVEY"}`
(`restrictions` and `sharing` optional; free text until the vocabularies are defined). An admin
sets it per source; a track several labelled sources report for carries the label of the
highest-priority one.

`publisher` names the instance (`node_id`), its release (`version`) and the version of the
correlation engine that produced the track (`correlation`; see
[algorithms.md](algorithms.md)). Tracks that only sensors report for are never published:
a message always has a feed that may stand alone behind it.

### Track management

These are present only when they apply:

| Field | |
|---|---|
| `kind` | `group` for a group a track manager formed (a battle group, a flight, a convoy); absent for a track |
| `members` | a group's member tracks (`tms-<UID>`) |
| `groups` | the groups a track belongs to (`tms-<UID>`) |
| `paired_with` | tracks a track manager paired with this one (GOLD PAIR: the same object, kept as separate tracks) |

A group is a track of its own, with its own track number: it sits at the centre of its live
members, with their mean course and speed and a `circular_error_m` that covers them all, and
carries the symbol the track manager built for it (2525C: a naval task organisation such as
`SHSPGG----` or the members' shared function, with the task-force indicator in position 11 and the
echelon in position 12). Without live members it stays, `lost`. Dissolving it publishes a
`delete`; its members stay.

A track manager's merge (GOLD MRG) looks like any merge: the surviving track keeps its number and
the other is deleted.

### `sidc`

Every track carries a symbol code with its standard, so the consumer knows how to draw it:

| `standard` | `code` example | |
|------------|----------------|-|
| `2525c` | `SFSPCLDD-------` | MIL-STD-2525C, 15 characters (upper case, `-` padded) |
| `2525d` | `10033000001211000000` | MIL-STD-2525D, 20 digits |
| `cot` | `a-f-S-C-L` | Cursor-on-Target type |

A feed may supply any of the three; OpenTrack recognises the standard from the code's shape. When
no feed supplies one, it is the CoT type OpenTrack derives from `domain` and `affiliation`, so the
field is always present. An affiliation set by policy or an operator is written into the code's
standard identity, so `sidc`, `affiliation` and `force_code` always agree.

### Where attribute values come from

For each output schema field, OpenTrack takes the value from, in order:

1. **The track**, under `ext.<field>`: a feed's value, through the source's mapping to that
   field, or the value of the registry entity the track resolves to (through its identifiers),
   where the source's pipeline links an entity attribute to the field. The entity is the
   authority: when a feed reports something different, the entity's value is published and
   OpenTrack shows operators the difference. The same holds for the OTH-GOLD fields a pipeline
   links (by default the entity's name, class name, domain, affiliation, track type and symbol).
2. **OpenTrack itself**, for fields linked to a built-in value (lifecycle `state`, `speed_mps`,
   `identifiers`, `sources`, ...). `confidence` is the probability that the track is a real
   object, 0 to 1: `1 − Π(1 − existence · pairing confidence)` over its source tracks, where a
   tracker's tracks carry their existence probability and track feeds count as 1 unless they
   report their own (see [algorithms.md](algorithms.md), correlation-3).

### Non-point contacts

Two optional fields carry evidence that isn't a point (see [non-point-contacts.md](non-point-contacts.md)):

- **`area`**: when the track's position comes from an area of uncertainty. It is either:
  - `{"polygon": [[lat, lon], …]}`, the area as reported; or
  - `{"ellipse": {"semi_major_m", "semi_minor_m", "orientation_deg"}}`, the 1-sigma error ellipse around `lat`/`lon`, when the error is over 2 km.

  Draw the area rather than a point.
- **`bearings`**: lines of bearing that point at the track, the latest from each sensor. Each gives:
  - the sensor's position (`latitude`, `longitude`);
  - `bearing_deg` (degrees true) and `sigma_deg` (one standard deviation);
  - measured `range_m` and `range_sigma_m`, if known;
  - `max_range_m`, if known;
  - `residual_deg`: how far the line misses the track;
  - the emitter's `identifiers`, such as an ELNOT.

A track made from cross-fixed bearings is an ordinary track. Its `provenance` names the `crossfix` sensor and the sensors whose bearings made it.

### Bearings no track took: `contacts.bearing.<source>.<key>`

A line of bearing that points at no single track is published live on core NATS: it isn't stored in a stream, and it goes stale in about a minute. It is either cross-fixed into a track or given up.

```json
{"schema": "opentrack.contact.v1", "op": "bearing", "source_id": "esm-a", "source_track_key": "e1",
 "observed_at": "2026-09-27T12:00:04Z", "latitude": 50.6, "longitude": -1.6,
 "bearing_deg": 47.5, "sigma_deg": 1.0, "identifiers": [{"scheme": "elnot", "value": "A123"}]}
```

Subscribe to `contacts.>` to draw them.

## `delete`

The track was retired (dropped after going stale, merged away, or deleted by an operator). The
consumer removes it. The delete stays in the stream as that subject's latest message until it
ages out, so a consumer that starts later still learns of it.

```json
{
  "schema": "opentrack.track.v2",
  "op": "delete",
  "track_id": "tms-OTK000000001",
  "uid": "OTK000000001",
  "reason": "no report for 21600s",
  "deleted_at": "2026-09-25T02:34:54.426550476Z",
  "publisher": { "node_id": "opentrack-OTK", "version": "0.3.1", "correlation": "correlation-5" }
}
```

When the track was merged into another, `merged_into` names the survivor (`"merged_into":
"tms-OTK000000007"`, with `reason` `"merged into tms-OTK000000007"`). A consumer that keeps
references to tracks (a selection, a note, a history) should move them there. With several
OpenTrack nodes sharing one picture, this is how two numbers for one object become one.

## `delete_history_point`

A track manager deleted a bad point from the track's position history. The consumer drops the
track's point at `observed_at` from what it shows of the track's trail. When it was the track's
latest point, an `upsert` follows with the track back at the point before.

Each deletion has its own subject, `tracks.history.tms-<UID>.<Unix ms of the point>`. It never
replaces the track's own latest message, and the stream keeps every deletion until it ages out.

```json
{
  "schema": "opentrack.track.v2",
  "op": "delete_history_point",
  "track_id": "tms-OTK000000001",
  "uid": "OTK000000001",
  "observed_at": "2026-09-26T21:40:03.120Z",
  "position": { "latitude": 33.0, "longitude": -117.0 },
  "reason": "a GPS jump",
  "decision_id": 4812,
  "deleted_at": "2026-09-26T21:41:10.004Z",
  "publisher": { "node_id": "opentrack-OTK", "version": "0.3.1", "correlation": "correlation-5" }
}
```

**Consumers must ignore what they do not know.** Take a message's kind from `OT-Op` (or `op`), and
skip any kind or subject you don't handle. A consumer of `tracks.>` that reads every message as a
track update should filter on `OT-Op: upsert` and `delete`, or subscribe to `tracks.tms-*` only.

## Other subjects

Nothing else OpenTrack publishes falls under `tracks.>`. Per-source raw output, when an admin
enables it with recorded consent, goes to `opentrack.raw.<source>` (or an admin-chosen subject
outside `tracks.>`): core NATS (no stream, no acknowledgement), one message per observation as the
source's pipeline produced it, before correlation, as JSON (`source_id`, `source_track_key`,
`observed_at`, `position`, `kinematics`, `classification`, `identifiers`, `ext`, ...). It is a
diagnostic and integration feed, not a contract: its shape follows OpenTrack's internal
observation and can change between versions.

## Permissions OpenTrack needs

Publish on `tracks.>` and on any raw-output subjects, plus the JetStream API for the stream
(`$JS.API.STREAM.INFO.TRACKS`, and `$JS.API.STREAM.CREATE.TRACKS` unless the stream is created
beforehand). Authenticate with `OT_NATS_CREDS` (credentials file), `OT_NATS_TOKEN`, or
`OT_NATS_USER` and `OT_NATS_PASSWORD`.
