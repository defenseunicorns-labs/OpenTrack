import { useEffect, useMemo, useState } from 'react'
import { TbArrowMerge, TbLink, TbLinkOff, TbSearch, TbTrash, TbUsersGroup, TbX } from 'react-icons/tb'
import { setWorkerUrl } from 'maplibre-gl'
import maplibreWorkerUrl from 'maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url'
import { Badge, Button, CollapsiblePanel, DataTable, FieldSelect, Input, Modal, useToast, type DataTableColumn } from '@kineticlogic/staresdk'
import type { MapFitTo, MapLine, MapPoint } from '@kineticlogic/staresdk/map-view'
import { DEFAULT_BEARING_RANGE_M, bearingLine, bearingWedge } from '../../lib/geodesy'
import { api, type Entity, type TrackRow } from '../../api/client'
import { ago, errorMessage, fmtNum, STATE_COLOR } from '../../lib/format'
import { affiliationColor, EVIDENCE_COLOR, UNCERTAINTY_COLOR } from '../../lib/palette'
import { symbolUrl } from '../../lib/symbol'
import { EntityEditor } from '../registry/EntityEditor'
import { InfoTip } from '../../components/InfoTip'
import { GroupEditor } from './GroupEditor'
import { TrackCard } from './TrackCard'
import { ManagementLog } from './ManagementLog'
import { TrackMap, type TrackMapPoint } from './TrackMap'
import type { HistoryPoint, SystemTrack } from '../../api/client'
import { useCan } from '../../auth/context'
import { useBasemapTiles } from '../../lib/basemap'
import { FILL_PANEL, usePanelOpen } from '../../lib/panelOpen'
import { FILL_TABLE, useFillHeight } from '../../lib/fillHeight'

// MapLibre resolves its worker relative to its own module, which a bundle breaks.
setWorkerUrl(maplibreWorkerUrl)

const OUTLINES = '/world-110m.geo.json'
const REFRESH_MS = 5000

// A closed panel in the map row keeps its title's height instead of stretching to its neighbour.
const CLOSED_IN_ROW = { alignSelf: 'start' } as const
// The map row shares the height left over with the track table; its panels' bodies keep a minimum.
const FILL_ROW = { flex: '1 1 0' } as const

const displayName = (t: TrackRow) => t.name ?? t.callsign ?? (t.gold_name !== 'UNKNOWN' ? t.gold_name : '')

const COLUMNS: DataTableColumn<TrackRow>[] = [
  {
    key: 'symbol',
    header: '',
    width: 34,
    render: (t) => {
      const url = symbolUrl(t.sidc, 18)
      return url ? <img className="symbol-cell" src={url} alt="" /> : null
    },
  },
  { key: 'track', header: 'Track', mono: true, width: 140, render: (t) => t.track_id, sortValue: (t) => t.track_id },
  {
    key: 'links',
    header: 'Group / pair',
    width: 110,
    render: (t) =>
      t.kind === 'group' ? (
        <Badge color="blue" size="sm" title={`${t.members?.length ?? 0} members`}>
          group · {t.members?.length ?? 0}
        </Badge>
      ) : (
        <span className="num-row">
          {(t.groups ?? []).length > 0 && (
            <Badge color="grey" size="sm" title={`Member of ${(t.groups ?? []).join(', ')}`}>
              in group
            </Badge>
          )}
          {(t.paired_with ?? []).length > 0 && (
            <Badge color="grey" size="sm" title={`Paired with ${(t.paired_with ?? []).join(', ')}`}>
              paired
            </Badge>
          )}
        </span>
      ),
    sortValue: (t) => (t.kind === 'group' ? 2 : (t.groups ?? []).length + (t.paired_with ?? []).length > 0 ? 1 : 0),
  },
  { key: 'name', header: 'Name', render: displayName, sortValue: (t) => displayName(t) || null },
  { key: 'marking', header: 'Marking', mono: true, width: 150, render: (t) => t.marking ?? '', sortValue: (t) => t.marking ?? null },
  { key: 'domain', header: 'Domain', width: 90, render: (t) => t.domain, sortValue: (t) => t.domain },
  { key: 'affiliation', header: 'Affiliation', width: 100, render: (t) => t.affiliation, sortValue: (t) => t.affiliation },
  {
    key: 'force',
    header: 'Force',
    width: 60,
    align: 'right',
    mono: true,
    render: (t) => String(t.force_code).padStart(2, '0'),
    sortValue: (t) => t.force_code,
  },
  {
    key: 'state',
    header: 'State',
    width: 100,
    render: (t) => (
      <Badge color={STATE_COLOR[t.state] ?? 'grey'} size="sm" uppercase>
        {t.state}
      </Badge>
    ),
    sortValue: (t) => t.state,
  },
  {
    key: 'entity',
    header: 'Entity',
    width: 100,
    render: (t) =>
      t.notices > 0 ? (
        <Badge color="warning" size="sm">
          replaced {t.notices}
        </Badge>
      ) : t.entity_id ? (
        <Badge color="blue" size="sm">
          entity
        </Badge>
      ) : (
        ''
      ),
    sortValue: (t) => (t.notices > 0 ? 2 : t.entity_id ? 1 : 0),
  },
  { key: 'sources', header: 'Sources', mono: true, render: (t) => t.sources.join(', '), sortValue: (t) => t.sources[0] },
  {
    key: 'speed',
    header: 'Speed',
    width: 80,
    align: 'right',
    mono: true,
    render: (t) => fmtNum(t.speed_mps, 1, ' m/s'),
    sortValue: (t) => t.speed_mps,
  },
  {
    key: 'last',
    header: 'Last seen',
    width: 80,
    align: 'right',
    render: (t) => ago(t.last_seen),
    sortValue: (t) => new Date(t.last_seen).getTime(),
  },
]

/** A filter select whose first option means "no filter". */
function Filter({ label, any, values, value, onChange }: { label: string; any: string; values: string[]; value: string | null; onChange: (v: string | null) => void }) {
  return (
    <FieldSelect
      ariaLabel={label}
      fields={[{ name: any }, ...values.map((name) => ({ name }))]}
      value={value ?? any}
      onChange={(v) => onChange(v === any || v === null ? null : v)}
      style={{ width: 128 }}
    />
  )
}

const distinct = (rows: TrackRow[], pick: (t: TrackRow) => string[]) => [...new Set(rows.flatMap(pick))].sort()

/** Every live track on a map and in a table, with the selected one's details and entity. */
export default function TrackDbPage({ selected, onSelect }: { selected: string; onSelect: (uid: string) => void }) {
  const tiles = useBasemapTiles()
  const { toast } = useToast()
  const [logRev, setLogRev] = useState(0)
  const [trail, setTrail] = useState<{ uid: string; points: HistoryPoint[] } | null>(null)
  // The selected track as its card loaded it: its bearings and area.
  const [selTrack, setSelTrack] = useState<SystemTrack | null>(null)
  // The track whose history is drawn on the map: off until asked, and off again for the next track.
  const [trailOn, setTrailOn] = useState<string | null>(null)
  // Zoom to track requests: the key counts clicks, so each click moves the map once.
  const [zoom, setZoom] = useState<{ uid: string; n: number } | null>(null)
  const canManage = useCan('track_manager')
  const [rows, setRows] = useState<TrackRow[] | null>(null)
  const [query, setQuery] = useState('')
  const [state, setState] = useState<string | null>(null)
  const [domain, setDomain] = useState<string | null>(null)
  const [affiliation, setAffiliation] = useState<string | null>(null)
  const [source, setSource] = useState<string | null>(null)
  const [editing, setEditing] = useState(false)
  // Tracks ticked for track management, by track id, in the order ticked.
  const [checked, setChecked] = useState<string[]>([])
  const [merging, setMerging] = useState<string | null>(null)
  const [grouping, setGrouping] = useState<{ id: string | null; members: string[] } | null>(null)
  const [busy, setBusy] = useState(false)
  const { confirm } = useToast()
  const [mapOpen, setMapOpen] = usePanelOpen('ot.panel.trackmap')
  const [cardOpen, setCardOpen] = usePanelOpen('ot.panel.trackcard')
  const [tableOpen, setTableOpen] = usePanelOpen('ot.panel.tracktable')
  const [tableFill, tableHeight] = useFillHeight()

  useEffect(() => {
    let cancelled = false
    let warned = false
    const load = () =>
      api.tracks().then(
        (r) => {
          if (!cancelled) setRows(r.tracks)
          warned = false
        },
        (e) => {
          if (!cancelled && !warned) toast({ variant: 'error', title: 'Tracks unavailable', message: errorMessage(e) })
          warned = true
        },
      )
    load()
    const t = setInterval(load, REFRESH_MS)
    return () => {
      cancelled = true
      clearInterval(t)
    }
  }, [toast])

  const all = useMemo(() => rows ?? [], [rows])
  const shown = useMemo(() => {
    const q = query.trim().toLowerCase()
    return all.filter((t) => {
      if (state && t.state !== state) return false
      if (domain && t.domain !== domain) return false
      if (affiliation && t.affiliation !== affiliation) return false
      if (source && !t.sources.some((s) => s.startsWith(`${source}/`))) return false
      if (!q) return true
      const hay = [t.track_id, t.name, t.callsign, t.gold_name, t.class, t.sidc.code, ...t.sources, ...(t.identifiers ?? []).map((i) => `${i.scheme}:${i.value}`)]
      return hay.some((h) => h && h.toLowerCase().includes(q))
    })
  }, [all, query, state, domain, affiliation, source])

  const points: TrackMapPoint[] = useMemo(
    () =>
      shown.map((t) => ({
        id: t.uid,
        latitude: t.latitude,
        longitude: t.longitude,
        color: affiliationColor(t.affiliation),
        label: `${t.track_id} ${displayName(t)}`.trim(),
        // On the map: the name, else the track number.
        text: displayName(t) || t.track_id,
      })),
    [shown],
  )

  const row = all.find((t) => t.uid === selected) ?? null
  const here: [number, number] | null = row ? [row.longitude, row.latitude] : null

  // The selected track's history as a line in its colour, oldest to newest, ending where it is now.
  const trailCoords: [number, number][] = useMemo(() => {
    if (!selected || trailOn !== selected || !trail || trail.uid !== selected) return []
    const c = trail.points.map((p): [number, number] => [p.lon, p.lat])
    const last = c[c.length - 1]
    if (here && (!last || last[0] !== here[0] || last[1] !== here[1])) c.push(here)
    return c
    // `here` is a fresh array each render; its values are what matter.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, trailOn, trail, here?.[0], here?.[1]])
  const trailColor = row ? affiliationColor(row.affiliation) : undefined
  // Non-point evidence of the selected track, split so operators can independently
  // hide evidence, sensor positions, and uncertainty.
  const mapEvidence = useMemo(() => {
    const evidence: MapLine[] = []
    const uncertainty: MapLine[] = []
    const sensors: MapPoint[] = []
    if (!selTrack || selTrack.uid !== selected) return { evidence, uncertainty, sensors }
    const sensorIds = new Set<string>()
    for (const b of selTrack.bearings ?? []) {
      const range = b.range_m ?? b.max_range_m ?? DEFAULT_BEARING_RANGE_M
      const id = `bearing-${b.source_id}-${b.source_track_key}`
      if (!sensorIds.has(b.source_id)) {
        sensorIds.add(b.source_id)
        sensors.push({
          id: `sensor-${b.source_id}`,
          latitude: b.latitude,
          longitude: b.longitude,
          color: EVIDENCE_COLOR,
          label: b.source_id,
        })
      }
      if (b.sigma_deg > 0)
        uncertainty.push({
          id: `${id}-wedge`,
          coordinates: bearingWedge(b.latitude, b.longitude, b.bearing_deg, b.sigma_deg, range),
          color: UNCERTAINTY_COLOR,
          width: 1.25,
          opacity: 0.65,
        })
      evidence.push({ id, coordinates: bearingLine(b.latitude, b.longitude, b.bearing_deg, range), color: EVIDENCE_COLOR, width: 2.5, dashed: true })
    }
    if (!here) return { evidence, uncertainty, sensors }
    const g = selTrack.view.geometry
    let ring: [number, number][] = []
    if (g?.type === 'area') ring = g.polygon.map(([lat, lon]): [number, number] => [lon, lat])
    else {
      const e = selTrack.view.uncertainty?.ellipse
      if (e) {
        const ellipse = ellipseRing(here[1], here[0], e.semi_major_m, e.semi_minor_m, e.orientation_deg)
        if (ellipse.length >= 3)
          uncertainty.push({ id: 'position-uncertainty', coordinates: [...ellipse, ellipse[0]], color: UNCERTAINTY_COLOR, width: 1.5, dashed: true })
      }
    }
    if (ring.length >= 3) evidence.push({ id: 'area', coordinates: [...ring, ring[0]], color: trailColor, width: 2, dashed: true })
    return { evidence, uncertainty, sensors }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selTrack, selected, here?.[0], here?.[1], trailColor])
  const lines: MapLine[] = useMemo(
    () => (trailCoords.length >= 2 ? [{ id: 'history', coordinates: trailCoords, color: trailColor, width: 3 }] : []),
    [trailCoords, trailColor],
  )
  const fitTo: MapFitTo | undefined = useMemo(() => {
    if (!zoom || zoom.uid !== selected) return undefined
    // The whole line when it is shown (close in if it is short), else where the track is now.
    if (trailCoords.length) return { key: zoom.n, coordinates: trailCoords, maxZoom: 15, padding: 48 }
    return here ? { key: zoom.n, coordinates: [here], maxZoom: 11 } : undefined
    // Only a click moves the map; data changes under the same key are ignored by MapView anyway.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [zoom])
  const byId = useMemo(() => new Map(all.map((t) => [t.track_id, t])), [all])
  const ticked = checked.filter((id) => byId.has(id))
  const tickedRows = ticked.map((id) => byId.get(id)!)
  const tickedGroups = tickedRows.filter((t) => t.kind === 'group')
  const tickedTracks = tickedRows.filter((t) => t.kind !== 'group')
  const toggle = (id: string) => setChecked((c) => (c.includes(id) ? c.filter((x) => x !== id) : [...c, id]))
  const columns: DataTableColumn<TrackRow>[] = useMemo(
    () => [
      {
        key: 'tick',
        header: '',
        width: 34,
        render: (t) => (
          <input
            type="checkbox"
            className="tick"
            aria-label={`Select ${t.track_id}`}
            checked={checked.includes(t.track_id)}
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => e.stopPropagation()}
            onChange={() => toggle(t.track_id)}
          />
        ),
      },
      ...COLUMNS,
    ],
    [checked],
  )
  const reload = () => {
    setLogRev((n) => n + 1)
    return api.tracks().then((r) => setRows(r.tracks), () => {})
  }
  const run = async (what: string, f: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await f()
      setChecked([])
      await reload()
    } catch (e) {
      toast({ variant: 'error', title: `Not ${what}`, message: errorMessage(e) })
    } finally {
      setBusy(false)
    }
  }
  const label = (t: TrackRow) => `${displayName(t) || t.track_id}`
  const pair = () =>
    run('paired', async () => {
      await api.pairTracks(ticked)
      toast({ variant: 'success', title: 'Paired', message: tickedRows.map(label).join(', ') })
    })
  const merge = (into: string) =>
    run('merged', async () => {
      for (const from of ticked.filter((id) => id !== into)) await api.mergeTracks(from, into, true)
      setMerging(null)
      toast({ variant: 'success', title: 'Merged', message: `into ${label(byId.get(into)!)}` })
    })
  const doNotPair = () =>
    run('recorded', async () => {
      await api.doNotPair(ticked[0], ticked[1])
      toast({ variant: 'success', title: 'Do not pair', message: tickedRows.map(label).join(', ') })
    })
  const remove = async () => {
    const ok = await confirm(
      `Delete ${ticked.length} track${ticked.length === 1 ? '' : 's'}? ${tickedGroups.length ? 'Groups are dissolved (their members stay). ' : ''}They are deleted downstream; a source still reporting starts a new track.`,
      { title: 'Delete tracks', confirmLabel: 'Delete' },
    )
    if (ok) await run('deleted', () => api.deleteTracks(ticked))
  }
  const group = () => {
    if (tickedGroups.length === 1 && tickedTracks.length > 0) {
      const g = tickedGroups[0]
      void run('added', async () => {
        await api.groupMembers(g.track_id, tickedTracks.map((t) => t.track_id), [])
        toast({ variant: 'success', title: 'Added to group', message: `${tickedTracks.length} to ${label(g)}` })
      })
    } else if (tickedGroups.length === 1) {
      setGrouping({ id: tickedGroups[0].track_id, members: [] })
    } else {
      setGrouping({ id: null, members: tickedTracks.map((t) => t.track_id) })
    }
  }
  const select = (uid: string | null) => {
    setEditing(false)
    onSelect(uid ?? '')
  }
  const filtered = !!(query.trim() || state || domain || affiliation || source)

  return (
    <>
      {/* The map row and the track table share the window; the log follows, a scroll below. */}
      <div className="panels tight fill-page">
        <div className="workspace fill" style={mapOpen || cardOpen ? FILL_ROW : undefined}>
          <CollapsiblePanel
            title="Track map"
            badge={rows ? `${points.length.toLocaleString()} shown` : undefined}
            open={mapOpen}
            onOpenChange={setMapOpen}
            style={mapOpen ? undefined : CLOSED_IN_ROW}
          >
            <div className="workspace-body map">
              <TrackMap
                aria-label="Live tracks"
                points={points}
                sensorPoints={mapEvidence.sensors}
                lines={lines}
                evidenceLines={mapEvidence.evidence}
                uncertaintyLines={mapEvidence.uncertainty}
                selectedId={selected || null}
                onSelect={select}
                outlines={OUTLINES}
                tiles={tiles}
                fitKey="tracks"
                fitTo={fitTo}
                height="100%"
              />
            </div>
          </CollapsiblePanel>
          <CollapsiblePanel
            title="Track card"
            open={cardOpen}
            onOpenChange={setCardOpen}
            style={cardOpen ? undefined : CLOSED_IN_ROW}
          >
            <div className="workspace-body">
              {selected ? (
                <TrackCard
                  key={selected}
                  uid={selected}
                  onHistory={(points) => setTrail({ uid: selected, points })}
                  onTrack={setSelTrack}
                  historyOnMap={trailOn === selected}
                  onHistoryOnMap={(on) => setTrailOn(on ? selected : null)}
                  onZoom={() => setZoom((z) => ({ uid: selected, n: (z?.n ?? 0) + 1 }))}
                  onChanged={() => void reload()}
                  onEdit={
                    row
                      ? () => (row.kind === 'group' ? setGrouping({ id: row.track_id, members: [] }) : setEditing(true))
                      : undefined
                  }
                  editLabel={row?.kind === 'group' ? 'Edit group' : row?.entity_id ? 'Edit entity' : 'Edit'}
                />
              ) : (
                <div className="panel-body">
                  <span className="muted">Select a track on the map or in the table to see its details.</span>
                </div>
              )}
            </div>
          </CollapsiblePanel>
        </div>

        <CollapsiblePanel
          title="Tracks"
          badge={rows ? (filtered ? `${shown.length.toLocaleString()} of ${all.length.toLocaleString()}` : all.length.toLocaleString()) : undefined}
          open={tableOpen}
          onOpenChange={setTableOpen}
          style={tableOpen ? FILL_PANEL : undefined}
          titleActions={
            <>
            <InfoTip label="Track columns">
              Group / pair: a group track and its member count, or a track that is in a group or paired with others (hover the badge for
              which). Force: the OTH-GOLD force code (0-39) for its domain and affiliation, e.g. 09 friendly surface, 07 hostile surface, 32
              unknown. State: tentative until it has 3 reports (the server default); confirmed; lost when unreported for a while (60 s for air,
              15 min surface and land, 30 min subsurface) until a report brings it back; dropped after 6 h without one (default), when it
              leaves the table and is deleted downstream. Entity: resolves to a registry
              entity; replaced N: the entity overrode N values its feeds report. Sources: each contributing source as source/its track
              key.
            </InfoTip>
            <div className="search">
              <TbSearch aria-hidden />
              <Input
                aria-label="Search tracks"
                placeholder="Search"
                style={{ paddingLeft: 26 }}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
            </div>
            <InfoTip label="Search">
              Matches any part of the track number, name, callsign, GOLD name, class, SIDC, a source/key, or an identifier as scheme:value
              (e.g. mmsi:235009870). Case does not matter.
            </InfoTip>
            </>
          }
          actions={
            <>
              <Filter label="State" any="any state" values={distinct(all, (t) => [t.state])} value={state} onChange={setState} />
              <Filter label="Domain" any="any domain" values={distinct(all, (t) => [t.domain])} value={domain} onChange={setDomain} />
              <Filter label="Affiliation" any="any affiliation" values={distinct(all, (t) => [t.affiliation])} value={affiliation} onChange={setAffiliation} />
              <Filter label="Source" any="any source" values={distinct(all, (t) => t.sources.map((s) => s.split('/')[0]))} value={source} onChange={setSource} />
            </>
          }
        >
          <div className="panel-fill" ref={tableFill}>
            <div className="selection-bar">
              {ticked.length === 0 ? (
                <>
                  <span className="muted">Tick tracks to pair, merge, group or delete them.</span>
                  <InfoTip label="Track management">
                    Pair: the same object, kept as separate tracks. Merge: one track survives with the others&apos; history and sources, and
                    correlation never splits it again. Do not pair (two tracks): different objects, which correlation never pairs or merges; undo it
                    in the log below. Group: a battle group, flight or convoy published as a track of its own at its members&apos;
                    centre. Delete: the track is deleted downstream.
                  </InfoTip>
                  <span className="spacer" />
                  <Button size="sm" variant="ghost" disabled={shown.length === 0} onClick={() => setChecked(shown.slice(0, 200).map((t) => t.track_id))}>
                    Select shown
                  </Button>
                  <InfoTip label="Select shown">Ticks the tracks the table shows after search and filters, up to 200 of them.</InfoTip>
                </>
              ) : (
                <>
                  <strong>{ticked.length} selected</strong>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<TbLink />}
                    disabled={!canManage || busy || tickedTracks.length < 2 || tickedGroups.length > 0}
                    onClick={pair}
                  >
                    Pair
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<TbArrowMerge />}
                    disabled={!canManage || busy || tickedTracks.length < 2 || tickedGroups.length > 0}
                    onClick={() => setMerging(ticked[0])}
                  >
                    Merge
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<TbLinkOff />}
                    disabled={!canManage || busy || tickedTracks.length !== 2 || tickedGroups.length > 0}
                    onClick={doNotPair}
                  >
                    Do not pair
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    icon={<TbUsersGroup />}
                    disabled={!canManage || busy || tickedGroups.length > 1}
                    onClick={group}
                  >
                    {tickedGroups.length === 1 ? (tickedTracks.length ? 'Add to group' : 'Edit group') : 'Group'}
                  </Button>
                  <Button size="sm" variant="ghost" icon={<TbTrash />} disabled={!canManage || busy} onClick={remove}>
                    Delete
                  </Button>
                  <span className="spacer" />
                  <Button size="sm" variant="ghost" icon={<TbX />} onClick={() => setChecked([])}>
                    Clear
                  </Button>
                </>
              )}
          </div>
          <DataTable
            aria-label="Tracks"
            columns={columns}
            rows={shown}
            rowKey={(t) => t.uid}
            selectedKey={selected || null}
            onRowClick={(t) => select(t.uid)}
            defaultSort={{ key: 'last', direction: 'desc' }}
            maxHeight={tableHeight ?? 'none'}
            style={FILL_TABLE}
            empty={rows === null ? 'LOADING…' : filtered ? 'No track matches.' : 'No live tracks. Enable a source to start ingesting.'}
          />
          </div>
        </CollapsiblePanel>
      </div>

      <div className="panels tight after-fill">
        <ManagementLog rev={logRev} onChanged={() => void reload()} />

        {merging !== null && (
          <Modal title="Merge tracks" onClose={() => setMerging(null)} width={480} resizable={false}>
            <div className="panel-body">
              <span className="muted">
                The surviving track keeps its number and takes the others&apos; history, sources, groups and pairings; the others are deleted
                downstream. Correlation never splits the merged track.
              </span>
              <div className="stack" style={{ gap: 6 }} role="radiogroup" aria-label="Surviving track">
                {tickedTracks.map((t) => (
                  <label key={t.track_id} className="radio-row">
                    <input type="radio" name="survivor" checked={merging === t.track_id} onChange={() => setMerging(t.track_id)} />
                    <span className="mono">{t.track_id}</span>
                    <span>{displayName(t)}</span>
                    {t.published === false && <span className="muted">not published</span>}
                  </label>
                ))}
              </div>
              <div className="num-row" style={{ justifyContent: 'flex-end' }}>
                <Button size="sm" variant="ghost" onClick={() => setMerging(null)}>
                  Cancel
                </Button>
                <Button size="sm" icon={<TbArrowMerge />} disabled={!canManage || busy} onClick={() => merge(merging)}>
                  Merge into {merging}
                </Button>
              </div>
            </div>
          </Modal>
        )}

        {grouping && (
          <GroupEditor
            groupId={grouping.id}
            members={grouping.members}
            rows={all}
            open
            onClose={() => setGrouping(null)}
            onSaved={(id) => {
              setChecked([])
              void reload()
              if (id) setGrouping({ id, members: [] })
            }}
          />
        )}

        {selected && row && row.kind !== 'group' && (
          <EntityEditor
            entityId={row.entity_id}
            seed={{
              name: displayName(row) || null,
              domain: (row.domain !== 'unknown' ? row.domain : null) as Entity['domain'],
              affiliation: (row.affiliation !== 'unknown' ? row.affiliation : null) as Entity['affiliation'],
              // The track's identifiers, so its later tracks find the entity (graded against the name or
              // callsign they broadcast); saving also pins the entity to this track (pinTrack).
              identifiers: (row.identifiers ?? []).map((i) => ({ ...i, expected_name: row.name ?? row.callsign ?? null })),
            }}
            pinTrack={row.track_id}
            open={editing}
            onClose={() => setEditing(false)}
            onSaved={() => {}}
          />
        )}
      </div>
    </>
  )
}

/** An error ellipse as a ring of [lon, lat] points (semi-axes in metres, orientation degrees true). */
function ellipseRing(lat: number, lon: number, a: number, b: number, orientationDeg: number): [number, number][] {
  const R = 6371008.8
  const t = (orientationDeg * Math.PI) / 180
  const out: [number, number][] = []
  for (let i = 0; i < 48; i++) {
    const u = (2 * Math.PI * i) / 48
    const [x, y] = [a * Math.cos(u), b * Math.sin(u)]
    const n = x * Math.cos(t) - y * Math.sin(t)
    const e = x * Math.sin(t) + y * Math.cos(t)
    out.push([lon + ((e / (R * Math.cos((lat * Math.PI) / 180))) * 180) / Math.PI, lat + ((n / R) * 180) / Math.PI])
  }
  return out
}
