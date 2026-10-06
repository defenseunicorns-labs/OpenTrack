# OpenTrack operator guide

For track managers and viewers: the people who watch the picture and keep it right. Installing,
configuring and securing OpenTrack is in the [administrator guide](admin.md). Written for
OpenTrack **0.4.7**.

Every section has a short, stable anchor, so the ⓘ tips in the UI can link to it (in the app:
`#help/operator/<anchor>`). An anchor is the heading's slug, by the rule GitHub uses: lowercase,
every character that is not a letter, digit, space, hyphen or underscore removed, and spaces
turned into hyphens. Anchors are unique within a guide.

## Getting started

OpenTrack takes in feeds (AIS, ADS-B, radar, GMTI, ESM…), makes **tracks** from them, fuses every
source's tracks of one object into one **system track**, and publishes the picture to OpenStare
and other consumers. Your job is to keep that picture right: say which tracks are the same object
and which are not, say what an object is, and remove what is wrong.

A few words used throughout:
- **Source track:** one source's own track of an object (an AIS vessel, a radar's track 17).
- **System track** (or just track): OpenTrack's track of an object, fed by one or more source
  tracks. It has a **track number** such as `tms-OTK000000042`: a site code and a 9-digit
  sequence. Consumers know the track by that number.
- **Entity:** a real-world object in the registry (a ship, an aircraft), with identifiers such as
  an MMSI or ICAO address. Tracks that carry its identifiers resolve to it.
- **Published:** sent to consumers. Not every track is (see [Not published](#not-published)).

### Signing in

Open OpenTrack in a browser and sign in with your email and password, or with the single
sign-on button your site offers ("Sign in with SSO", or "Sign in with OpenStare" when OpenTrack
runs beside OpenStare). If your site shows a warning notice after sign-in, **AGREE** to go on;
**DECLINE** signs you out.

A bar at the top and bottom of every page shows the system's classification, when your site sets
one.

After you sign in, a notice says when you last signed in and how many failed attempts there have
been since. If you don't recognise them, tell your administrator.

OpenTrack's security rules (fixed, from the DoD application security STIG) ask this of you:
- **Change your password.** When your password has expired, or an administrator has just set it,
  OpenTrack asks for a new one before anything else. It needs 15 characters or more, with upper
  and lower case, a digit and a special character. It can't be one of your last five, and at
  least 8 characters must differ from the old one.
- **Sign in again after a break.** A session you haven't used for 15 minutes ends (10 for
  admins), and every session ends 24 hours after you signed in.
- **Wait after failed attempts.** Three wrong passwords within 15 minutes lock your account for 15
  minutes. An administrator can unlock it sooner.
- **See your sessions** under **Sessions** in the account menu (top right), and end any you don't
  recognise.

### Roles

What you can do depends on your role. Buttons you can't use are greyed out.

| | Viewer | Track manager |
|---|---|---|
| See tracks, the map, track cards, provenance and history | yes | yes |
| See sources, correlation, the registry, the schema and settings | yes | yes |
| Export tracks (GeoJSON, CSV) and the registry (XLSX, CSV) | yes | yes |
| Pair, unpair, merge, split, delete and group tracks | | yes |
| Designate tracks: edit their entity, pin an entity | | yes |
| Accept and reject correlation suggestions | | yes |
| Undo track management decisions | | yes |
| Delete history points | | yes |
| Create, edit, delete and import registry entities | | yes |

Admins can also change sources, the output schema, correlation settings, instance settings,
plugins and accounts.

### Your account

The button with your name, top right, shows how you signed in and your role. From it:
- **Change password**, when you signed in with a password. Your other sessions end.
- **Sign out.**

The sun and moon button beside it switches between the dark and light themes.

### Links

The address bar follows where you are, so a reload or a shared link lands on the same screen:
`#tracks/<uid>` opens a track, `#sources/<id>` a source, `#help/operator/<anchor>` a section of
this guide.

## Overview

The **Overview** tab shows how OpenTrack is doing:
- **System status:** the server, its algorithms, the database, Redis and NATS (the link to
  consumers). Green is working; red shows the error. If NATS is red, nothing reaches consumers:
  tell an admin.
- **Throughput:** per minute over the last hour: what came in, what correlation did (applied,
  new tracks, paired, proposed or split, ended), and what went out.
- **Tracks:** live tracks by state and domain, how many have an entity, and how many publish an
  entity's value in place of what their feed reports ("entity differs").
- **Backlog and resources:** a backlog that keeps growing means OpenTrack is falling behind its
  feeds.

## Sources

The **Sources** tab shows the feeds OpenTrack takes in:
- **Topology:** each source's pipeline, left to right, into correlation and out to consumers.
  Rates are per minute, averaged over 10 minutes. Moving dashes carry data; grey dashed links belong
  to disabled sources. Click a source to open it.
- **Sources:** each source's state and throughput. States: **disabled** (switched off),
  **starting**, **running** (connected and reading), **connecting** (not connected yet, no errors),
  **failing** (not connected and hitting errors; it keeps retrying). **Last frame** is how long ago a
  message last arrived.
- A source's **Status**, **Transport**, **Pipeline** and **History** tabs show its errors,
  activity, settings and saved revisions.

A source that stops reporting leaves its tracks to go lost and then drop. If one fails, tell an
admin; only admins change sources.

## Correlation

The **Correlation** tab shows what the engine proposes, how it pairs, and every decision it and
operators made. A number on the tab tells track managers how many suggestions wait.

### Suggestions

The engine asks a person when it isn't allowed, or isn't sure enough, to act alone:
- **PAIR** "*B* into *A*": the two tracks look like the same object. Pairings wait here when
  correlation runs in **suggest** mode.
- **SPLIT** "*source track* off *A*": a source track has stopped agreeing with the track it
  reports for, and may be following another object.

**Evidence** says why. Click a track's name to open it in Track Management. The status filter
shows open, accepted, rejected, expired or all suggestions.

### Accept or reject

Track managers only.

- **Accept a pair suggestion:** *B* is merged into *A*. *A* keeps its number and takes *B*'s
  source tracks and history; *B* is deleted downstream. It is your decision, like a
  [merge](#merge) you make in the table: correlation never splits it. To take a source track off
  again, use [Split](#split) or [undo](#undo) it.
- **Reject a pair suggestion:** the two are recorded as different objects (**do not pair**), and
  correlation won't pair them again.
- **Accept a split suggestion:** the source track leaves for a track of its own, and the two are
  not paired again.
- **Reject a split suggestion:** the source track stays. The engine won't propose that split again
  for 30 minutes.

Each is a decision in the log. All but a rejected split can be [undone](#undo).

### Expired suggestions

A suggestion **expires** when a track it names no longer exists: it was merged, deleted, dropped or
purged before anyone decided. There is nothing left to decide; if the tracks come back and still
agree, the engine proposes again. Accepting a suggestion whose track has just gone expires it too,
with "out of date: a track no longer exists".

### Correlation decisions

The **Decisions** panel lists pairings by identifier, merges, splits, do-not-pair rules, rejected
splits, source tracks that ended, retired tracks and settings changes, by the engine or by people. Click a row to see its evidence: the probabilities
and comparisons behind it. The **Settings** panel shows how the engine correlates; only admins
change it.

## Track Management

The **Track Management** tab is where you work on the picture: the map, the selected track's
card, the tracks table and the management log.

### Track map

Every track the table shows, coloured by affiliation, with its number and name. Click a track to
select it; its card opens beside the map. Filtering the table filters the map.

A selected track can also show:
- its **history** as a line in its colour, oldest to newest (History tab → **Show on map**);
- its **bearing lines** and **area** ([Bearings and areas](#bearings-and-areas)).

**Zoom to track** (History tab) moves the map to it.

The two buttons at the map's top right hide or show the basemap, and open **Map display**:
basemap dimming, track size (compact, standard, large), **track labels** (each track's name, or
its track number when it has none; labels that would overlap are left out, more appear as you
zoom in, and the selected track's is always shown), high-contrast tracks (an outline round
each track and a halo round the selected one), and whether to show the selected track's evidence
lines, its sensors' locations and its uncertainty, and whether to fade the other tracks while one
is selected. Your choices are kept in this browser.

### Bearings and areas

Passive sensors (ESM, direction finding) report a line of bearing, not a position; ELINT reports an
area of uncertainty. OpenTrack fuses them into tracks. On the map, for the selected track only:
- **Bearing lines:** a dashed gold line from each sensor (a gold dot) that currently has a bearing
  on the track, out along the bearing to the range it measured, else its maximum range (250 km
  when the sensor doesn't give one), inside a pale outline of its ± error wedge. A sensor that
  measures range as well as bearing (an acoustic array, say) places the object itself. The track is somewhere on or near each line: several
  lines crossing at the track are a cross-fix, the sensors agreeing on where the emitter is. A line
  that passes well away from the track is weak evidence.
- **Area:** a dashed outline in the track's colour, for the area an ELINT report gave. The object
  is somewhere inside, most likely near the middle. An area joins a ship's or a fix's track when
  that track carries the area's emitter identity (ELNOT), on its own lines say, and fits it clearly
  best; a wide area in open water with nothing but its position to go on stays a track of its own.
- **Uncertainty:** the track's position error ellipse, a pale dashed outline.

The track card's **Bearings** row lists each bearing: sensor, bearing and its error (±), how far
it misses the track, and any emitter identity (ELNOT). A bearing never moves a track by itself; it
adds evidence and identity. Bearings no track takes aren't shown on the map. See
[docs/non-point-contacts.md](../non-point-contacts.md).

### Track table

Every live track, newest report first. Columns:

| Column | |
|---|---|
| (symbol) | Its MIL-STD-2525 symbol. |
| Track | Its track number. |
| Group / pair | **group · n** for a group of n members; **in group** and **paired** for tracks that are. |
| Name | The name, callsign, or published name. |
| Domain, Affiliation | Where it operates, and friend, hostile, neutral, unknown… |
| Force | The OTH-GOLD force code, from domain and affiliation. |
| State | tentative, confirmed or lost ([Track states](#track-states)). |
| Entity | **entity** when it resolves to a registry entity; **replaced n** when the entity replaced n values its feeds report. |
| Sources | Its source tracks, as `source/key`. |
| Speed, Last seen | |

**Search** matches a track number, name, callsign, class, symbol code, source or identifier
(`mmsi:366…`). The filters narrow by state, domain, affiliation and source. The badge shows how
many match.

Tick tracks to act on them ([Managing tracks](#managing-tracks)). **Select shown** ticks the first
200 shown; **Clear** unticks all.

### Track card

The selected track's card. Its head shows the symbol, name and number (after the track's marking,
when it has one), and badges:
- **FILTERED** or **NOT PUBLISHED** when consumers don't get it ([Not published](#not-published));
- its state.

**Edit** (track managers) designates the track ([Designate](#designate)). It reads **Edit entity**
when the track has one, and **Edit group** on a group. The card has three tabs: Details,
Provenance and History.

### Details tab

What the track publishes:

| Field | |
|---|---|
| Marking | The track's security marking, when its sources carry a label ([Markings](#markings)). |
| Class-name | The OTH-GOLD class and name, as published. |
| Force code | The two-digit OTH-GOLD force code, with the domain and affiliation it comes from. |
| Track type | tactical, live training, simulated training or demand entry. |
| SIDC | The symbol code and its standard (MIL-STD-2525C, 2525D or a CoT type). |
| Time, Position | When and where it was last observed, as published. |
| Course / speed | |
| Subject | The NATS subject consumers read it on, `tracks.tms-<number>`. |
| Bearings | Lines of bearing reporting for it ([Bearings and areas](#bearings-and-areas)). |
| Reported by | With several OpenTrack nodes sharing the picture: the node that reports this track to the others. |
| Entity | The registry entity it resolves to, or none. |
| Members | A group's member tracks. |
| Groups | The groups it belongs to. |
| Paired with | The tracks paired with it, each with an unpair button ([Unpair](#unpair)). |

Below them, a warning line for each value the entity replaced ("*key*: *source* reports *x*; the
entity's *y* is published"), then the **Attributes** the output schema publishes, each with where
its value came from (OpenTrack or the entity) when that is known.

### Provenance tab

How the track came to be:
- **Source tracks:** each source track reporting for it, with
  - **Paired by:** how it joined: *started this track*, *shared* an identifier (such as MMSI),
    *merged: agreed kinematically…* with the evidence, *split off another track*, or *plots
    associated* for a detection source's plots;
  - **Confidence:** the probability it is the same object as the rest, then (after the dot) its
    existence ([Confidence and existence](#confidence-and-existence));
  - **Last:** when it last reported;
  - **Split** (track managers, when there are two or more): see [Split](#split).
- The heading also gives the track's own **confidence**.
- **Lineage:** a graph of the track's source tracks, merges, pairings, groups and entity over time.
- **Correlation decisions:** every link in the track's graph, live (blue) or ended (grey), with the
  decision that made it, when, and by whom. A live link a person made has an **undo** button.
- **First seen**, **Last seen** and **Observations**: how many reports it has taken.

### Confidence and existence

- **Existence** (per source track): the probability that the source track is a real object and not
  clutter, from how well its plots fit and how often it goes unseen. Trackers set it; a feed that
  reports tracks (AIS, ADS-B) counts as 100%.
- **Confidence** (per source track): the probability that it is the same object as the rest of the
  system track: 100% for the source track that started it; for the others, how sure the pairing
  was.
- **Track confidence:** the probability the system track is a real object: at least one of its
  source tracks is real and belongs to it, 1 − Π(1 − existence × confidence). A plot-only
  contribution doesn't count.

A low track confidence means it stands on weak evidence: a single uncertain radar track, say.

### Track states

- **tentative:** new; it hasn't had enough reports to be confirmed (3 by default, fewer for some
  sources). Tentative tracks aren't published.
- **confirmed:** a real track.
- **lost:** no report for a while (60 s for air, 15 minutes for surface and ground, 30 minutes for
  subsurface). It is still published, as lost, at its last position.
- A track with no report for 6 hours (the site may set otherwise) is **dropped**: it leaves the
  table and is deleted downstream.

### Not published

Consumers get a track only when it is authoritative. The card says why one isn't:
- **NOT PUBLISHED:** one of
  - it is not confirmed yet;
  - only sources that may not stand alone report for it (radar, GMTI and other plot sources, by
    default: they wait for a track feed such as AIS to report for the same object);
  - its entity's **Publish** is set to **never**.
- **FILTERED:** the output filter (Correlation settings) holds it back: by area, affiliation,
  domain, track type, confidence or a rule. Hover the badge for the reason. A published track that
  becomes filtered is deleted downstream until it passes again.

An entity's **Publish: always** publishes its tracks at once, whatever the rules and the filter say
([Publish override](#publish-override)).

### History tab

The track's published positions, newest first: time, latitude, longitude, course and speed. They
are kept for 12 hours by default, at most one every 10 seconds (the site may set otherwise).
- **Show on map** draws them as a line; it is off for each track until you turn it on.
- **Zoom to track** moves the map to the line, or to the track.
- The bin deletes a bad point ([History points](#history-points)).

## Managing tracks

Track managers only. Every action here is a decision: it goes in the log with your name, and
most can be [undone](#undo). Correlation leaves your pairs, merges, splits and do-not-pair rules
alone.

### Pair

Pair tracks that are the same object but should stay separate tracks (say, two sensors' tracks
consumers want to keep apart). Each lists the others as **Paired with**.

To pair tracks:
1. Tick two or more tracks (not groups) in the table.
2. Choose **Pair**.

### Unpair

To unpair two tracks: open either one's card, and in **Paired with** choose the unpair button next
to the other.

### Merge

Merge tracks that are the same object and should be one track (GOLD MRG).

To merge tracks:
1. Tick two or more tracks (not groups).
2. Choose **Merge**.
3. Pick the **surviving track**. It keeps its number and takes the others' history, source tracks,
   groups and pairings. The others are deleted downstream. Tracks not published yet say so.
4. Choose **Merge into**.

Correlation never splits a track you merged, here or by accepting a pair suggestion. To take a
source track off it again, use [Split](#split) or [undo](#undo) the merge.

### Split

When a track has two or more source tracks and one of them belongs to another object:
1. Open the track's card, **Provenance** tab.
2. In **Source tracks**, choose **Split** on the one that doesn't belong, and confirm.

It leaves for a new track of its own (the message gives its number), and the two are not paired
again.

### Do not pair

A **do not pair** rule records that two tracks are different objects, so correlation never pairs
or merges them, even when they share an identifier. You make one by:
- ticking exactly two tracks (not groups) in the table and choosing **Do not pair**, before
  correlation proposes them;
- **rejecting** a pair suggestion ([Accept or reject](#accept-or-reject));
- **splitting** a source track off ([Split](#split)).

The rule is between their source tracks, so it holds for them whatever track they report for
later. It doesn't stop you pairing or merging them yourself. It shows in the
[management log](#management-log) as **Do not pair**, and can be [undone](#undo) there. (The API
also takes it directly: `POST /api/v1/tracks/do-not-pair`.)

### Delete

To delete tracks:
1. Tick them.
2. Choose **Delete**, and confirm.

They are deleted downstream. A ticked group is dissolved; its members stay. A source still
reporting for a deleted track starts a new track, with a new number. To remove a whole source's
tracks for good, ask an admin to disable the source.

### Groups

A group is a carrier battle group, a flight or a convoy, published as a track of its own at the
centre of its live members, with their mean course and speed and an uncertainty that covers them
all. Members stay published and list the group.

To form a group:
1. Tick its member tracks.
2. Choose **Group**. The group editor opens.
3. Give it a **Name** (such as `CSG 12`), a **Class** (published as the OTH-GOLD class-name, such
   as `CARRIER STRIKE GROUP`), a **Domain** and an **Affiliation**.
4. Pick its **Symbol**: a **Base** (a naval task organisation icon, or the function the members
   share, so a flight of bombers keeps the bomber icon), an **Echelon** and **Task force**. The
   SIDC is built from them; turn on **by hand** to type any 2525C, 2525D or CoT code.
5. Save.

To add tracks to a group, tick the group and the tracks, and choose **Add to group**. To remove
members, change the group or dissolve it, tick only the group and choose **Edit group** (or
**Edit group** on its card): remove members with their ✕, and dissolve it with the bin. Dissolving
a group deletes it downstream; its members stay.

### Designate

Designating says what a track is: its name, class, domain, affiliation, symbol and attributes.
It is done through the track's **entity**.

To designate a track:
1. Select it and choose **Edit** (or **Edit entity**) on its card.
2. Set the fields ([Entity editor](#entity-editor)), such as **Affiliation: hostile**.
3. **Save**.

What that changes depends on how the track finds its entity:
- **A track no identifier names** (radar, GMTI, a fix): the new entity is **pinned** to this
  track, by the identifier `track` = its number. Everything you set applies to the track at once,
  whatever reports for it, and follows it through merges. So "mark this radar track hostile" is
  **Edit**, **Affiliation: hostile**, **Save**.
- **A track with identifiers** (AIS, ADS-B): the entity gets those identifiers (MMSI, ICAO…) and
  is pinned to this track as well. The pin makes it apply to this track at once, whatever its feeds
  report. The identifiers let the same ship or aircraft's later tracks find it; there its values
  replace the feed's when the name or callsign it broadcasts corroborates the match, and only for
  the fields the source's pipeline links from the entity (ask an admin which).

Where an entity's value replaces what a feed reports, the card shows the warning line and the
table shows **replaced n**.

### History points

To delete a bad position (a multipath jump, say):
1. Open the track's card, **History** tab.
2. Choose the bin on the point, give a reason if you like, and **Delete point**.

Consumers get the deletion too. When it was the track's latest point, the track steps back to the
point before and is republished. A deleted point can't be undone.

## Undo

Track managers can undo a pair, unpair, merge, split, do-not-pair, delete or group change (form,
change, members, dissolve), including an accepted suggestion and a rejected pair suggestion.

To undo a decision:
1. Find it in the **Track management log** (below the tracks table), or in the track's
   **Provenance** tab under **Correlation decisions**.
2. Choose **Undo**, and confirm.

The undo is a decision of its own. It reverses exactly what the decision did:
- a deleted or merged-away track comes back under its old number;
- a split source track goes back;
- a group formed is dissolved, a group dissolved is formed again, a group change is taken back.

An undo is refused while someone has since made another decision on the same tracks: undo that one
first. What the engine did on its own since doesn't block it. The engine's own decisions, deleted
history points and undos themselves can't be undone.

### Management log

The **Track management log** lists track management decisions, newest first: number, time, who
(with the other node's site code when it was made on another node), the decision, its tracks, the
reason, and its status (**undone by #n**, or **undoes #n**). It refreshes every 15 seconds; the
refresh button reloads it now.

### Markings

A source can carry a security label, and the tracks it reports carry it too. The track card and
the track table's **Marking** column show it as a portion marking, such as `(U//FOUO)` or
`(S//ORCON/REL TO USA, GBR)`:
- the classification (`U`, `C`, `S`, `TS`, or as written, such as `CUI`);
- after `//`, its restrictions (dissemination controls), separated by `/`;
- `REL TO` and the countries and organisations it may be released to; `NOFORN` when none.

A track reported by several labelled sources takes the highest classification, every
restriction, and only the countries all of them release to. No marking means the track's sources
carry no label: treat it as the system's classification, on the banner.

OpenTrack does not hide tracks by clearance: everyone signed in sees every track. Handle each one
as its marking says. TAK shows the same marking at the start of the track's remarks, and exported
files carry it.

## Export

Anyone can download the picture. **Settings → Data → Live tracks**:
- **GeoJSON:** every live track as a point feature, with the published fields and attributes, its
  state, confidence and source tracks.
- **CSV:** one row per track: track id, name, class, domain, affiliation, force code, track type,
  SIDC, time, latitude, longitude, state, confidence, whether it is published, sources, and the
  attributes as JSON, with each track's marking in a `classification` column first.

Each file is marked: the CSV's first line, above the header, and the GeoJSON's `security.marking`.
It is the highest marking of the tracks in it, or the banner's classification when none has a
label. Keep the file as its marking says.

Both include tracks that aren't published (see the `published` column). The registry exports from
the Registry tab ([Spreadsheets](#spreadsheets)).

## Registry

The **Registry** tab holds the entities: the real-world objects tracks resolve to. Each has
identifiers, a status, the OTH-GOLD minimum (name, class name, domain, affiliation, track type,
CoT type, SIDC) and free-form attributes.

### Entities

The table lists name, identifiers, domain and affiliation, attributes, when it was saved, and
status. **Search** matches part of a name or identifier value, or an exact entity id. It shows the
first 200; search to narrow. Click a row to open the entity. **New entity** (track managers) makes
one.

Status:
- **active:** tracks resolve to it by its identifiers.
- **retired:** kept on record with its history, but no track resolves to it. Its identifiers stay
  reserved to it: delete them, or the entity, to use them again.

### Entity editor

- **Identity:** the OTH-GOLD minimum:
  - **Name:** published in capitals in place of the feed's name. Blank leaves the feed's.
  - **Class name:** such as the ship or aircraft class.
  - **Domain**, **Affiliation:** blank leaves the feed's value. Together they set the force code.
  - **Track type:** tactical (the default), live training, simulated training, demand entry.
  - **Domain, Affiliation, CoT type, SIDC** follow each other: change the affiliation and both
    symbol codes take it; change the domain to another than the symbol's and the symbol becomes
    that domain's generic one; type a SIDC (2525D; a 2525C code is stored as its 2525D equivalent)
    or a CoT type and the other code, the domain and the affiliation follow.
  - **CoT type**, **SIDC:** the symbol. The SIDC wins; blank, it comes from the CoT type. The
    flag button beside SIDC opens the **symbol designer**: choose the affiliation, then the symbol
    set, entity, type and subtype (MIL-STD-2525D point symbols; no lines or areas), with a live
    preview. **Use symbol** fills both fields: the 2525D SIDC, and the matching CoT type (from the
    standard's 2525C equivalent; a symbol with none, such as a control measure, gets its
    affiliation and dimension only, `a-f-X`).
- **Status:** active or retired.
- **Identifiers:** scheme (`mmsi`, `imo`, `icao`, `callsign`, `hull`, `elnot`, `cot-uid`, `track`,
  or any other), value, and the **Name** the track is expected to report under it, which
  corroborates the match. An identifier belongs to one entity. An entity needs at least one.
- **Attributes:** key, type (text, number, boolean, datetime in RFC 3339, or json) and value. A
  value that doesn't parse as its type is refused on save.
- **Live tracks:** the tracks resolving to it now; **Replaced** counts the values it replaced
  (hover for them).
- **History:** every saved revision: when, by whom, and what changed.

**Save** applies it to its live tracks at once. The bin deletes the entity.

### Publish override

Top right of the entity editor, **Publish**:
- **automatic:** the usual rules ([Not published](#not-published)).
- **always:** its tracks are published at once, confirmed or not, whatever reports for them and
  whatever the output filter says.
- **never:** its tracks stay inside OpenTrack, and are withdrawn downstream if already published.

### Spreadsheets

To edit many entities at once:
1. **Export XLSX** (or **Export CSV**) to get the registry as a sheet: one row per entity, with
   `entity_id`, `name`, `status`, `publish`, the minimum (`class_name`, `domain`, `affiliation`,
   `track_type`, `cot_type`, `sidc`), `id:<scheme>` columns (several values separated by `;`) and
   `attr:<key>:<type>` columns.
2. Edit it in any spreadsheet program. Add rows for new entities.
3. **Import sheet** (track managers) and choose the file. OpenTrack shows what it would do, row by
   row: new, updated, unchanged, or an error.
4. **Apply**. Nothing is written while any row has an error: fix them and import again.

A row updates the entity its `entity_id` names, else the one its identifiers belong to, else makes
a new one. Blank cells leave values as they are. Identifiers are only ever added, and never taken
from another entity.

## Schema

The **Schema** tab shows what every published track carries:
- **Always published:** the OTH-GOLD minimum (contact and position) and a symbol code.
- **Output schema versions:** the attributes on top. Tracks publish the newest published version.
  Admins edit a draft and publish it.
- **Set by feeds, not published:** values sources set that no published field carries.

Consumers read the full message contract in [docs/nats-output.md](../nats-output.md).

## Settings

Everyone sees **Settings**; only admins change it. Operators use it for:
- **Data export:** live tracks as GeoJSON or CSV, and the registry ([Export](#export)).
- The site name, the classification and warning banners, how long position history is kept, and
  the plugins installed.

## In TAK

When an admin sets up a TAK output (Settings → TAK output), the tracks OpenTrack publishes also
appear in TAK (ATAK, WinTAK, iTAK, and TAK Server's users), as the same tracks OpenStare gets:
- Each track is a TAK marker named by its callsign or name, or its OpenTrack track number
  (`OTK000000042`) when it has neither. Its uid is `tms-<UID>`, the id OpenStare uses.
- Its symbol follows its SIDC, or its affiliation and domain: a hostile air track is a red air
  symbol, an unknown track with no domain an unknown ground symbol. Designating a track here
  changes its symbol in TAK within a few seconds.
- Its details show course and speed, the position error as its circular error, and (when the admin
  turned remarks on) the track number and the sources reporting it. A track with a security label
  always has its marking at the start of its remarks ([Markings](#markings)).
- It moves as OpenTrack updates it (at most every few seconds). A track that ends, is deleted or
  merged away, or stops being published disappears from TAK at once. A track that stops
  reporting goes stale in TAK a minute (by default) after its last report and disappears, even
  while OpenTrack still shows it as lost; it comes back when it reports again. TAK shows each
  track's time as its last report, not when it was sent.
- Only tracks: no lines of bearing, areas, history or group drawings. TAK users' own markers and
  positions do not come back into OpenTrack.

## Common questions

**Why isn't my track going to OpenStare?** Look at its card's badges: see
[Not published](#not-published).

**Why isn't my track in TAK?** TAK gets exactly the published tracks: if it is not published it is
not in TAK either ([Not published](#not-published)). If it is published, ask an admin to check the
TAK output's status in Settings ([In TAK](#in-tak)).

**Two tracks are one ship. Pair or merge?** Merge, as a rule: one track, one number. Pair when both
tracks must stay (two sensors' views consumers want to compare).

**The engine keeps pairing two different objects.** Split the wrong source track off
([Split](#split)); the two won't be paired again.

**I merged the wrong tracks.** Undo the merge from the management log ([Undo](#undo)).

**I set a track hostile, but it still shows unknown.** The entity is matched by identifiers, and
the source doesn't link affiliation from the entity. Pin it to the track instead
([Designate](#designate)), or ask an admin to link the field.

**A track came back under a new number after I deleted it.** A source still reports it. Deleting
removes the track, not the object; ask an admin to change the source, or set the entity's
**Publish** to **never**.

**"The engine did not answer within 10 s".** The part of OpenTrack that owns the picture isn't
running or is stuck. Tell an admin.
