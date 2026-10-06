# OpenTrack sync messages: interface control document

Version 2. The implementation is `crates/ot-sync/src/wire.rs` and `crates/ot-sync/src/sign.rs`, and their tests pin the byte layouts below. Version 2 is version 1 with a signature on every message (0.4.5).

This document is for whatever carries OpenTrack's messages between nodes (the *networking package*). OpenTrack does no networking between nodes. It hands its messages to the package on the node's own NATS server, and receives other nodes' messages from the package the same way. The design is in [multi-node.md](multi-node.md).

## The boundary

| Direction | NATS subject | Meaning |
|---|---|---|
| out | `ot.sync.out.<kind>` | OpenTrack has a message for other nodes |
| in | `ot.sync.in.<kind>` | a message from another node, for OpenTrack |

`ot.sync` is the default prefix (`OT_SYNC_PREFIX`). `<kind>` is `report`, `attrs`, `decision`, `summary`, `want` or `release`.

An `out` message with the header `OT-To: <site code>` is for that node only; without it, it is for every node. The package delivers the payload unchanged on `in` at each destination. It may keep the same `<kind>`, or deliver everything on any `in` subject: OpenTrack reads the kind from the payload.

What OpenTrack needs from the package:
- **Payloads are opaque and at most 1024 bytes**, signature included. Deliver them byte for byte. The signature is in the payload, not in NATS headers, so a package that carries only payloads loses nothing.
- **Delivery may be lossy.** Messages may be dropped, duplicated, reordered or delayed. OpenTrack repairs what matters (decisions) itself, and everything else is replaced by the next message.
- **Nothing about the sender.** Every payload names its sending node by site code and is signed with that node's key (see *Signature*). OpenTrack checks the signature itself, so a package need not prove the sender, though it may (and should keep strangers off the link: a refused message still costs bandwidth).
- **Priority, when the link is short.** In order of importance: `decision`, `want`, `summary`, `release`, `report`, then `attrs`. `attrs` can be dropped first.
- **No echo needed.** OpenTrack ignores its own messages if they come back.

Expected volume per node (500 tracks at steady state):
- `report`: about 6–8 kbit/s (measured with `bench swarm`). It rises with manoeuvring targets and falls with fewer tracks.
- `summary`: a few bytes every 5 s.
- The rest: rare.

## Encoding

All integers are big-endian. A *site code* is 3 ASCII bytes, A–Z or 0–9. A *UID* (a track number, shown as `AAA000000042`) is 7 bytes: its site code, then its 9-digit sequence as a `u32`. A *short string* is a `u8` length followed by that many UTF-8 bytes. A *blob* is a `u16` length followed by that many bytes.

### Envelope (14 bytes)

| Offset | Size | Field |
|---|---|---|
| 0 | 2 | magic `"OT"` (`0x4F 0x54`) |
| 2 | 1 | version, `2`; receivers drop other versions |
| 3 | 1 | kind: 1 report, 2 attrs, 3 decision, 4 summary, 5 want, 6 release |
| 4 | 3 | sender's site code |
| 7 | 8 | sender's clock (HLC) when sent: Unix ms × 65536 + counter |

The body follows directly, then the signature trailer (below). Nothing else may follow the body.

### 1: `report`: tracks the sender reports

Body: `u8` count, then that many reports. The first 32 bytes of each report are:

| Offset | Size | Field |
|---|---|---|
| 0 | 7 | UID |
| 7 | 4 | `i32` estimate time, ms relative to the envelope clock's ms |
| 11 | 4 | `i32` latitude, 1e-7 degrees |
| 15 | 4 | `i32` longitude, 1e-7 degrees |
| 19 | 2 | `i16` altitude, metres MSL; `-32768` unknown |
| 21 | 2 | `u16` course, 0.01 degrees true; `65535` unknown |
| 23 | 2 | `u16` speed, 0.01 m/s; `65535` unknown |
| 25 | 2 | `u16` error ellipse semi-major axis, metres (95%); `65535` unknown or larger |
| 27 | 2 | `u16` semi-minor axis, metres |
| 29 | 1 | `u8` orientation of the major axis, degrees true, 0–179 |
| 30 | 1 | high 4 bits: track quality 0–15; low 4 bits: domain (0 unknown, 1 air, 2 surface, 3 subsurface, 4 ground, 5 space) |
| 31 | 1 | flags: bit 0, origin time follows; bit 1, identifiers follow; bits 2–3, state (0 tentative, 1 confirmed, 2 lost, 3 dropped) |

Two optional parts follow, in this order:
- **Origin time** (flag bit 0): an `i64`, Unix ms at which the node that minted the UID created the track. OpenTrack sends it with every report, because it decides which of two numbers for one object survives, and a node must not decide that on a guess.
- **Identifiers** (flag bit 1): a `u8` count, then that many pairs of short strings, `(scheme, value)`, e.g. `("mmsi", "235009876")`.

Track quality grades the position error. 15 means 10 m or better, and each step down doubles the error. Of the nodes that see a track, the one with the best quality reports it (see *reporting responsibility* in the design).

A report carries only what the sender's own sensors say, never what it heard from other nodes.

### 2: `attrs`: a track's descriptive attributes

Body: `u8` count, then that many items. Each item is a UID and a blob holding a JSON object:
- `name`, `callsign`, `classification`: when known;
- `attributes`: the fields of the output schema, as published on `tracks.>` (see [nats-output.md](nats-output.md)).

It is sent when these change, and can be dropped first.

### 3: `decision`: track-management decisions

Body: `u8` count, then that many blobs. Each blob is a JSON object:

```json
{
  "id": "AAA:12",
  "hlc": 117309456384000000,
  "actor": "tm@example.org",
  "role": "track_manager",
  "command": {"op": "pair", "tracks": ["tms-AAA000000001", "tms-BBB000000007"]}
}
```

- `id` is the deciding node's site code and a sequence that has no gaps on that node.
- `hlc` is the deciding node's clock, which orders conflicting decisions.
- `command.op` is one of:
  - `pair`, `unpair`, `merge`, `do_not_pair`, `delete`
  - `group_create`, `group_update`, `group_members`, `group_dissolve`
  - `delete_history_point`, `undo`
  - `profile_schema`, `profile_correlation`

A node may relay decisions another node made.

### 4: `summary`: what the sender holds

Body:
- `u32`: the number of tracks the sender reports;
- `u8` count, then that many pairs of (site code, `u32` highest decision sequence held from that site).

It is sent every 5 s.

### 5: `want`: what the sender lacks

Body:
- `u8` flags: bit 0 asks the receiver for a snapshot (a `report` of every track it reports);
- `u8` count, then that many site entries. Each is a site code, then a `u8` range count, then that many (`u32` first, `u32` last) inclusive ranges of that site's decision sequences.

It is sent to the node whose summary showed the gap (`OT-To`), which answers with `decision` messages, also addressed.

### 6: `release`: tracks the sender stops reporting

Body: `u16` count, then that many UIDs. It is sent when a node leaves, so that others take its tracks over at once instead of after two missed heartbeats (24 s).

## Signature

Every message ends with a 68-byte trailer:

| Offset from the end | Size | Field |
|---|---|---|
| −68 | 4 | key id: the first 4 bytes of SHA-256 of the sender's 32-byte Ed25519 public key |
| −64 | 64 | Ed25519 signature (RFC 8032, FIPS 186-5) |

The signature covers every byte before it: the envelope, the body and the key id. A node signs each message once, whoever it is for, so a broadcast costs one signature, not one per peer. The key id only tells a receiver which key was used (for a clear refusal after a key change); the signature decides.

Keys:
- Each node makes its key pair once, in the FIPS module, and keeps it in `sync.key` beside its database (readable by its owner only).
- A public key is written `ed25519:` then the 32 bytes in standard base64. People compare it by its fingerprint: the first 16 bytes of the SHA-256 of the key, in hex, in groups of four characters.
- An admin pins each peer's public key in **Settings → Nodes**.

A receiver accepts a message only if all of these hold:
1. its site code is in **Trusted nodes** and a key is pinned for it (otherwise *untrusted* or *no key*);
2. the signature verifies with that key (otherwise *bad signature*; logged, and audited as possible impersonation);
3. its envelope clock is within 5 minutes of the receiver's clock (otherwise *stale*);
4. the same signature has not been accepted before (otherwise *replayed*). Ed25519 signatures are deterministic, so a copy of a message has the same signature. Receivers remember accepted signatures for 10 minutes, twice the clock window, so a copy is refused whenever it arrives.

Refused messages are counted in the link status (Settings → Nodes), and logged at most once a minute per site.

Replays beyond the message: decisions carry global ids (`<site>:<seq>`), so a decision already held changes nothing, even inside a fresh message. A `want` answer is new messages with a new clock.

What a signature does not cover: a node may relay decisions another node made (anti-entropy), and those entries are not signed by the node that made them. A trusted node can therefore claim that another trusted node made a decision. It could make the same decision under its own name anyway, since any node decides, so this gives it nothing more.

## Change control

A change to any layout means a new version byte. Nodes of different versions don't understand each other, so upgrade a swarm together.
