# Multi-node OpenTrack: design

Status: **built** (0.3.2); the gates below are met.

## What we are building

Several OpenTrack nodes share one track picture, with no node in charge. The picture is the same everywhere: the same object has the same track number on every node, and an operator's decision made on one node holds on all of them.

It must work in two places:

| | Server sites | Drone swarm (the main design case) |
|---|---|---|
| Nodes | 2–10, long-lived | 2–50, join and leave |
| Links | WAN, reliable, Mbit/s | radio mesh (IP MANET), lossy, 100 kbit/s – 2 Mbit/s shared |
| Partitions | rare, hours | routine, seconds to minutes |
| Leader | none wanted | none possible |
| Operators | at every site | mostly none on the drones; at a ground node |

Designing for the swarm covers the sites as well. Sites only add more bandwidth and longer-lived nodes.

Out of scope for now:
- networking between nodes: an external package carries OpenTrack's sync messages (see Transport)
- sharing the registry or entities
- sharing source and tracker configuration
- data links (JREAP, Link 16): not yet on the roadmap, but this design borrows their rules

## Principles

1. **Each node publishes only what it knows first-hand.** A node sends what its own sources say about a track, never what it learned from a peer. That is how Link 16 avoids data incest: nobody's reports come back to them disguised as a second sensor.
2. **One reporter per track.** For each track, the node that sees it best (*reporting responsibility*, R2) sends it. The others hold it quietly, and take over when the reporter stops or another node sees it clearly better.
3. **Deterministic rules, not votes.** Wherever two nodes might decide differently, a rule every node computes the same way settles it. Examples: which of two track numbers survives, and which of two conflicting operator decisions wins. There is no leader election and no quorum.
4. **Decisions are the replicated state.** Positions are perishable and are re-sent all the time. Decisions (pair, split, do-not-pair, delete, group, undo) are durable, so each is a replicated log entry with a globally unique id.
5. **A node alone is a whole OpenTrack.** Losing every peer degrades the picture to that node's own sources. Nothing stops.

## Track numbers

A UID is already `<site code><9 digits>`, and each node has its own `OT_SITE_CODE`, so two nodes never mint the same number. What has to be solved is two numbers for one object (a *dual designation*): A tracks a ship as `AAA000000012` and B tracks it as `BBB000000007`.

When a node correlates a peer's track with one of its own, it merges them the way it already merges two system tracks. The survivor is chosen by a global rule, not the local one (published first, then oldest `first_seen`):

1. The track with the earlier **origin time** survives. Origin time is when the minting node created it, carried in every report.
2. If the times are equal, the lower UID string survives.

Every node that makes the same correlation picks the same survivor. The retired number goes out on NATS as a `delete` with `reason: "merged"` and a new `merged_into` field, so consumers can re-point their references. It is also remembered as an alias, so late reports under it still land.

## What goes over the link

### Track reports (perishable)

For each track a node holds R2 for, it sends a compact report of what its own sources say:

| field | bytes | |
|---|---|---|
| UID | 8 | site code and sequence, packed |
| time | 4 | ms since the node's epoch |
| lat, lon | 8 | 1e-7° fixed point |
| alt | 2 | |
| course, speed | 4 | |
| position error (major, minor, orientation) | 4 | ellipse, not covariance |
| track quality 0–15, domain, state, flags | 2 | |
| origin time | 4 | only in the first report after a change |
| identifiers | variable | only when they change |

That is about 36 bytes a report. The UID and origin time are OpenTrack's. The rest is the GOLD minimum.

**Rate is set by dead reckoning, not by a clock.** The reporter keeps a copy of the peers' prediction of each track. It sends a report when that prediction drifts from its own estimate by more than a threshold (default 50 m surface, 200 m air), and otherwise every 12 s as a heartbeat. The 12 s heartbeat is the Link 16 period.

A ship on a steady course costs one report every 12 s. At that rate, 500 tracks come to about 12 kbit/s.

Full attributes (the output schema's `attributes`) go on a separate, lower-priority subject, only when they change. On a starved link they are the first thing dropped.

### Decisions (durable)

Every decision a person makes, and every correlation a node makes between its track and a peer's, is a log entry with a global id `<site>:<seq>`. Each entry also carries:

- a hybrid logical clock (HLC) stamp
- the actor
- the targets, by UID
- the op
- the evidence

What the engine decides about its own sources stays local (its pairings, splits and lifecycle). Peers see only the result: which tracks the node reports and what they contain.

Entries are append-only. Undo is a new entry that names the one it undoes.

### Snapshots

Nodes send snapshots so that a joining or rejoining node catches up without replaying everything. On request, a node sends:
- the latest report of each track it holds R2 for
- its decision log since the HLC the requester last saw

## Reporting responsibility

Track quality (TQ) runs from 0 to 15. It comes from the track's position error, and a node's own sources set it; tracks it only hears about from peers don't count. The rules are Link 16's, simplified:

- **Claim.** A node reports a track when it holds one no one reports, or when its TQ beats the current reporter's by at least 2. The margin of 2 stops two nodes trading the track back and forth.
- **Yield.** A node stops reporting a track when it hears a report on the same track with a higher TQ, or an equal TQ from a lower site code.
- **Take over.** When the reporter misses two heartbeats (24 s), the node with the best TQ claims it. Several nodes may claim at once. The yield rule sorts that out within one report.
- **Leave.** A node shutting down cleanly sends a release for its tracks, so peers take them over at once instead of after 24 s.

Every node stays able to report every track it can see. R2 only decides who spends the bandwidth.

## Correlating a peer's track

A peer's report enters the engine as a source track of a built-in source, `peer:<site>`. From there it goes through the same kinematic and identifier pairing as any source, with two differences:

1. **It never reports for a system track of its own.** Either it pairs with a local track, or it becomes that peer's track in the local picture, under the peer's UID.
2. **Correlation is not replicated; its outcome converges anyway.** Each node pairs other nodes' tracks with its own using its own correlation, and a merge that involves another node's number keeps the survivor the global rule picks (earlier origin, then lower UID). The nodes reach the same result without exchanging the decision:
   - Say B merges A's `AAA000000012` with its own `BBB000000007`. B then reports the survivor.
   - A adopts B's report, and its own correlation makes the same pairing, with the same survivor.

   Only a report under a track's own number makes its sender that track's reporter. So while the nodes still disagree, both numbers stay reported, and the one that has not merged yet gets the data it needs to merge.

A peer's track that no local source sees is still in the local picture, and still published on this node's NATS output. Every node's consumers get the whole picture. A peer's track goes stale when its reporter stops and nobody takes it over, just as a local one does.

## Operator decisions everywhere

Every track-management op is replicated:
- pair and unpair
- merge and split
- do-not-pair
- delete
- groups
- designate
- delete a history point
- undo

A decision refers to tracks by UID and to source tracks by `<site>/<source>/<key>`, so it means the same thing everywhere.

When decisions conflict, rules settle them without coordination:

| Conflict | Rule |
|---|---|
| Two decisions on the same pair of tracks (pair vs do-not-pair) | later HLC wins |
| Delete vs anything | delete wins until undone |
| Two undos of one decision | idempotent |
| Decision about a track this node never saw | kept, applied if the track arrives, expires with retention |
| Operator vs engine | operator always wins; an engine correlation never overrides a person's decision, from any node |

A decision still names who made it. The Management log shows each one with its site. Undo keeps working as it does now: the log says what it reverses, and a later decision by a person on the same tracks blocks it. That check now looks at decisions from every node.

Accounts, sessions and the audit record stay per node. A user signs in to each node on its own: its session (idle timeout, limit per account, lockout counts) is known only where it began, and ends there. Each node keeps its own hash-chained audit record of what it recorded, replicated decisions included, and verifies only its own chain.

Any node's track managers decide for every node. Roles don't need to match across nodes: a node applies a replicated decision if the deciding node's role for that person allowed it. An admin can mark a node **receive-only**: it applies other nodes' decisions but doesn't accept track management locally. That suits a drone with no operator.

## Transport: OpenTrack owns the data, not the network

Networking between nodes belongs to an external package, which carries data over whatever UAS protocols the platform has. OpenTrack defines the sync messages and assumes only this of the link:

- messages may be lost, duplicated, reordered or delayed, and nodes come and go;
- the link carries opaque messages up to about 1 KB and says which node sent each one.

Every exchange is therefore **self-repairing**. Nothing depends on the link delivering everything.

| Message | Carries | If lost |
|---|---|---|
| `report` | a track report (above) | the next report or heartbeat replaces it |
| `attrs` | a track's attributes, when they change | re-sent with the next heartbeat after a change is lost |
| `decision` | one decision log entry | repaired by `summary` |
| `summary` | every few seconds: the node's site, its HLC, and the highest decision sequence it holds from each site | the next one |
| `want` | a request for a range of a site's decisions, or a snapshot | asked again |
| `release` | a node leaving gives up its tracks | the 24 s take-over |

Decision logs converge by anti-entropy. A node reads its peers' `summary` and asks with `want` for any site's decisions it is missing, from anyone who has them, not only the site that made them. A decision made on one drone can therefore reach a node it was never directly linked to. Duplicates are harmless because ids are global.

**The boundary with the package** is a pair of local message streams:
- `ot.sync.out.<kind>` for messages leaving the node, marked broadcast or addressed to one peer (for `want` replies);
- `ot.sync.in.<kind>` for messages arriving, with the sender's site.

In 0.3.2 these are subjects on the node's local NATS. A package bridges them to the network, and so does a small built-in bridge for server sites that links two NATS servers directly. The bridge signs in to each NATS server as OpenTrack's own NATS connection does: TLS with a CA and a client certificate, `.creds`, a user and password or a token, the same for every node or set per node (see the admin guide, [The bridge's NATS credentials and TLS](guides/admin.md#the-bridges-nats-credentials-and-tls)). Messages have a versioned binary encoding documented as an ICD (`docs/sync-icd.md`), so the package needs nothing from OpenTrack but that document.

The site code is the sender's identity, and OpenTrack proves it itself (from 0.4.5). Each node signs every message it sends with its own Ed25519 key, once, whoever it is for (68 bytes a message). A receiver accepts a message only if an admin has pinned a public key for its site code in **Settings → Nodes**, the signature verifies with it, its clock is within 5 minutes, and the same message was not accepted before. Everything else is refused, counted and logged; a trusted site's message that fails its signature is also audited, as possible impersonation. The package still keeps strangers off the link (NKeys, mTLS, radio crypto), since refused messages cost bandwidth. Details: [sync-icd.md, *Signature*](sync-icd.md#signature).

## Configuration travels as a swarm profile

The output schema and correlation settings are shared, so the same track has the same attributes and pairs by the same rules on every node. Each is a versioned document. A change made by an admin on any node is a decision like any other: it replicates, and the later HLC wins. Source, tracker and plugin configuration stay local, since they describe each node's own sensors. The registry and entities stay local for now.

## Time

HLC stamps order decisions. Track reports carry the sensor time. Nodes need clocks within about a second of each other. Drones have GPS time, and sites have NTP.

A node whose peers' reports keep arriving from the future (more than 2 s ahead) shows a clock warning. It still accepts them. A message whose clock is more than 5 minutes from the receiver's is refused: that bounds how long a recorded message could be replayed.

## How we will know it works

A new benchmark, `bench swarm`, runs 3–10 OpenTrack nodes on one machine.
- It splits a scenario's sources among them. For example, Solent AIS goes on one node, radar on another, and a mix on a third; Autoferry's sensors go one per node.
- They are linked through an emulated link that caps bandwidth, adds loss, duplication, reordering and latency, and partitions on a script. It stands in for the external networking package.

| Measure | Gate (proposed) |
|---|---|
| Same object, same number on every node (after 10 s) | ≥ 98% of truth objects |
| Dual designations open longer than 30 s | < 1% |
| Pairing accuracy vs one node holding every source | within 1 point of wrong pairings |
| Bandwidth per node, 500 tracks, steady | ≤ 20 kbit/s |
| Converged after a 5-minute partition heals | ≤ 30 s |
| Tracks echoed back to their origin as a second track | 0 |
| An operator decision on node A holds on every node | ≤ 2 s on a clean link |

### Measured (2026-09-27)

The gate run had 5 nodes and 600 targets, with 10% loss, 300 ms jitter and 2% duplicate messages. It cut 2 nodes off from 3 for 5 minutes. The result is in `results/swarm-20260927-063423-5n-gate4`.

| Measure | Result |
|---|---|
| Same object, same number on every node | 98.8% of seen targets (99.2% on a clean link) |
| Duplicate tracks | 0.76% |
| Converged after the partition healed | 2 s |
| Track reports per node | 6.9 kbit/s per 500 tracks |
| Echoes | none: a report carries only the sender's own sources, and a test checks it |

A thin link: each node capped at 16 kbit/s, 10% loss, a sending budget of 14 kbit/s. Result: coverage 98.4%, one number 96.6%. Sharing ~550 tracks takes about 80 s at the start, because it is paced.

Four findings from getting there are now rules:
- **The origin goes with every report.** Without it, a node that missed a track's first report could keep the other number in a merge.
- **A report under a number merged away here goes to the survivor.** Otherwise the number comes back as a new track.
- **A track held only from other nodes counts as current in kinematic comparisons.** Their estimate is re-sent whenever it drifts past the threshold, so its prediction is current, not a view already used.
- **A drift within the track's own error is not sent, and a budget orders the rest by urgency.** Without that, radar noise used up a thin link.

## Build order

1. **Decisions go global.** *(done)*
   - Global decision ids and HLC stamps; migration `0014_sync`.
   - Decisions refer to UIDs and site-qualified source tracks.
   - Conflict rules, and undo across nodes.
   - This is testable in-process with two engines.
2. **Sync messages.** *(done)*
   - The binary encoding and `docs/sync-icd.md`.
   - `summary`/`want` anti-entropy, snapshots, the trusted peer list, receive-only nodes.
   - The swarm profile.
3. **Peer tracks.** *(done)*
   - Peer source, dead-reckoning rate.
   - R2 claim, yield and take-over.
   - Global survivor rule, `merged_into` on `delete` (an ICD addition).
4. **Server-site bridge and `bench swarm`.** *(done)*
   - The built-in bridge between two NATS servers.
   - `bench swarm` with the link emulator, and the gates above.
   - `docker compose` with two nodes.
5. **UI.** None of this needs the UI, but it helps operators to see it. *(done)*
   - A Nodes panel: peers, link health, R2 counts, clock offset.
   - The track card shows the reporting node.
   - The Management log shows decisions from other nodes.

## Decided (2026-09-27)

- An external package does the networking; OpenTrack defines the data and the boundary.
- Any node's track managers decide for every node; an admin can make a node receive-only.
- The output schema and correlation settings travel as a swarm profile; the registry does not.

## Open question

**What does a consumer need when two numbers become one?** The proposal is `delete` with `merged_into`. OpenStare would need to honour it.
