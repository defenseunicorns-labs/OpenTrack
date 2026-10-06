import { lazy, Suspense, useEffect, useMemo, useState } from 'react'
import { TbAlertTriangle, TbArrowBackUp, TbArrowsSplit, TbPencil, TbUnlink } from 'react-icons/tb'
import { Badge, Button, DataTable, TabPanel, Tabs, useToast, type DataTableColumn } from '@kineticlogic/staresdk'
import { InfoTip } from '../../components/InfoTip'
import { api, type ExtensionField, type GraphEdge, type HistoryPoint, type SystemTrack, type TrackResponse } from '../../api/client'
import { ago, errorMessage, fmtNum, fmtTime, show, STATE_COLOR } from '../../lib/format'
import { standardName, symbolUrl } from '../../lib/symbol'
import { useCan } from '../../auth/context'
import { TrackHistory } from './TrackHistory'
import { opLabel, undoable, useUndo } from './undo'

// React Flow loads only when the Provenance tab is open.
const LineageGraph = lazy(() => import('./LineageGraph'))

const REFRESH_MS = 5000

const TABS = [
  { id: 'card', label: 'Details' },
  { id: 'provenance', label: 'Provenance' },
  { id: 'history', label: 'History' },
]

const HISTORY_REFRESH_MS = 15_000

type Contributor = SystemTrack['contributors'][number]

/** Plots a detection source's contributions are keyed under (see the engine). */
const DETECTIONS = '~detections'
const pct = (p: number) => `${Math.round(p * 1000) / 10}%`
const keyOf = (c: Contributor) => `${c.source_id}/${c.source_track_key}`

/** How a source track came to report for this track, from its live link in the graph. */
function pairedBy(edge: GraphEdge | undefined): string {
  if (!edge) return '—'
  const evidence = (edge.attrs.evidence ?? {}) as Record<string, unknown>
  if (evidence.rule === 'identifier') return `shared ${String(evidence.identity ?? 'identifier')}`
  switch (edge.decision_op) {
    case 'create_system_track':
      return 'started this track'
    case 'pair':
      return 'shared identifier'
    case 'merge': {
      // "a and b agreed kinematically in 4 of 4 comparisons (6 m apart, σ 9 m)" → the evidence part.
      const why = edge.decision_reason?.match(/(agreed kinematically.*|share .*)$/)?.[1]
      return why ? `merged: ${why}` : `merged (decision #${edge.decision_id})`
    }
    case 'split':
      return 'split off another track'
    default:
      return edge.decision_op
  }
}

/** Where a published attribute's value came from, where that is known. */
function origin(f: ExtensionField, replaced: boolean): string {
  if (f.builtin) return 'OpenTrack'
  return replaced ? 'entity' : ''
}

/**
 * Render with `key={uid}`, so switching tracks starts from a clean state.
 *
 * A system track's baseball card: the GOLD fields every published track carries (with its
 * MIL-STD-2525 symbol), then the output schema's attributes; and its provenance, the source
 * tracks and correlation decisions behind it.
 */
export function TrackCard({
  uid,
  onEdit,
  editLabel = 'Edit',
  onHistory,
  onTrack,
  historyOnMap,
  onHistoryOnMap,
  onZoom,
  onChanged,
}: {
  uid: string
  onEdit?: () => void
  editLabel?: string
  /** The track's position history, whenever it is (re)loaded (the map draws it as a line when asked). */
  onHistory?: (points: HistoryPoint[]) => void
  /** The track as loaded, for the map (its bearings and area). */
  onTrack?: (t: SystemTrack) => void
  /** The history is drawn on the map (the map's owner holds this). */
  historyOnMap?: boolean
  onHistoryOnMap?: (on: boolean) => void
  /** Move the map to the track. */
  onZoom?: () => void
  /** A decision here changed the picture (an undo, a deleted point). */
  onChanged?: () => void
}) {
  const [tab, setTab] = useState('card')
  const canManage = useCan('track_manager')
  const [data, setData] = useState<TrackResponse | null>(null)
  const [missing, setMissing] = useState<string | null>(null)
  const [edges, setEdges] = useState<GraphEdge[] | null>(null)
  const [fields, setFields] = useState<ExtensionField[]>([])
  const [edgesRev, setEdgesRev] = useState(0)
  const [splitting, setSplitting] = useState<string | null>(null)
  const { toast, confirm } = useToast()
  const undo = useUndo()
  const [history, setHistory] = useState<HistoryPoint[] | null>(null)
  const [historyRev, setHistoryRev] = useState(0)

  // Position history: for the History tab and the map's line.
  useEffect(() => {
    let cancelled = false
    const load = () =>
      api.trackHistory(uid).then(
        (r) => {
          if (cancelled) return
          setHistory(r.points)
          onHistory?.(r.points)
        },
        () => !cancelled && setHistory((h) => h ?? []),
      )
    load()
    const t = setInterval(load, HISTORY_REFRESH_MS)
    return () => {
      cancelled = true
      clearInterval(t)
    }
    // onHistory is the parent's; a new function each render must not reload.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [uid, historyRev])

  // Live state, refreshed.
  useEffect(() => {
    let cancelled = false
    const load = () =>
      api.track(uid).then(
        (r) => {
          if (cancelled) return
          setData(r)
          setMissing(null)
          onTrack?.(r.track)
        },
        (e) => !cancelled && setMissing(errorMessage(e)),
      )
    load()
    const t = setInterval(load, REFRESH_MS)
    return () => {
      cancelled = true
      clearInterval(t)
    }
  }, [uid])

  // Graph history, when provenance is shown.
  useEffect(() => {
    if (tab !== 'provenance') return
    let cancelled = false
    api.explain(uid).then(
      (r) => !cancelled && setEdges(r.edges),
      () => !cancelled && setEdges([]),
    )
    return () => {
      cancelled = true
    }
  }, [uid, tab, edgesRev])

  // The latest published output schema's fields.
  const entityId = data?.track.entity_id ?? null
  useEffect(() => {
    let cancelled = false
    api.schema().then(
      (o) => !cancelled && setFields(o.versions.find((v) => v.version === o.latest_published)?.fields ?? []),
      () => {},
    )
    return () => {
      cancelled = true
    }
  }, [])

  const notices = useMemo(() => {
    const out: Record<string, NonNullable<SystemTrack['notices']>> = {}
    for (const n of data?.track.notices ?? []) (out[n.key] ??= []).push(n)
    return out
  }, [data])

  if (!data) {
    return (
      <div className="panel-body">
        {missing ? <span className="muted">{missing}. The track may have been retired.</span> : <span className="muted">LOADING…</span>}
      </div>
    )
  }
  const t = data.track
  const m = data.message
  const own = t.contributors.filter((c) => c.source_track_key !== DETECTIONS)
  const live = new Map((edges ?? []).filter((e) => e.kind === 'REPORTS_FOR' && e.valid_to_ms === null).map((e) => [e.src_key, e]))
  const split = async (c: Contributor) => {
    const key = keyOf(c)
    const ok = await confirm(`${key} leaves this track for a track of its own, and the two are not paired again.`, {
      title: 'Split source track',
      confirmLabel: 'Split',
    })
    if (!ok) return
    setSplitting(key)
    try {
      const r = await api.splitTrack(t.uid, key)
      toast({ variant: 'success', title: 'Split', message: `${key} is now ${r.new_track}` })
      setData({ ...data, track: { ...t, contributors: t.contributors.filter((x) => keyOf(x) !== key) } })
      setEdgesRev((n) => n + 1)
    } catch (e) {
      toast({ variant: 'error', title: 'Not split', message: errorMessage(e) })
    } finally {
      setSplitting(null)
    }
  }
  const contributorColumns: DataTableColumn<Contributor>[] = [
    { key: 'track', header: 'Source track', mono: true, render: keyOf },
    {
      key: 'by',
      header: 'Paired by',
      render: (c) => {
        const text = c.source_track_key === DETECTIONS ? 'plots associated' : pairedBy(live.get(keyOf(c)))
        return <span title={text}>{text}</span>
      },
    },
    {
      key: 'conf',
      header: 'Confidence',
      width: 96,
      align: 'right',
      render: (c) => {
        if (c.source_track_key === DETECTIONS) return '—'
        const exists = c.existence != null ? `, exists ${pct(c.existence)}` : ''
        return (
          <span title={`Same object as the rest: ${pct(c.confidence)}${exists}`}>
            {pct(c.confidence)}
            {c.existence != null && <span className="muted"> · {pct(c.existence)}</span>}
          </span>
        )
      },
    },
    { key: 'last', header: 'Last', width: 56, align: 'right', render: (c) => ago(c.last_report) },
    {
      key: 'act',
      header: '',
      width: 84,
      align: 'right',
      render: (c) =>
        own.length > 1 && c.source_track_key !== DETECTIONS ? (
          <Button size="sm" variant="ghost" icon={<TbArrowsSplit />} disabled={!canManage || splitting !== null} onClick={() => split(c)} title="Split it off this track">
            Split
          </Button>
        ) : null,
    },
  ]
  const icon = symbolUrl(m.sidc, 36)
  const attrs = t.attributes ?? {}
  return (
    <div className="panel-body">
      <div className="card-head">
        {icon && <img src={icon} alt={`${m.affiliation} ${m.domain} symbol`} height={36} />}
        <div className="card-name">
          <strong>{m.name !== 'UNKNOWN' ? m.name : (t.view.callsign ?? m.name)}</strong>
          <span className="mono muted">
            {data.marking && <span className="mono">{data.marking} </span>}
            {m.track_id}
          </span>
        </div>
        <span className="spacer" />
        {t.filtered ? (
          <span className="num-row">
            <Badge color="warning" size="sm" uppercase>
              filtered
            </Badge>
            <InfoTip label="Filtered">
              Held back from publishing ({t.filtered}): by the output filter in the correlation settings, or because its entity is set
              never to publish. If it was published before, it has been withdrawn downstream.
            </InfoTip>
          </span>
        ) : (
          t.published === false && (
            <span className="num-row">
              <Badge color="grey" size="sm" uppercase>
                not published
              </Badge>
              <InfoTip label="Not published">
                Kept inside OpenTrack: it is not confirmed yet, or every source reporting for it has Publish its lone tracks set to no (a
                sensor needs a track feed to agree). An entity set to always publish overrides both. Once published, a track stays
                published.
              </InfoTip>
            </span>
          )
        )}
        <Badge color={STATE_COLOR[t.state] ?? 'grey'} size="sm" uppercase>
          {t.state}
        </Badge>
        {onEdit && canManage && (
          <Button size="sm" variant="secondary" icon={<TbPencil />} onClick={onEdit}>
            {editLabel}
          </Button>
        )}
      </div>
      <Tabs aria-label="Track card views" idPrefix="trk" size="sm" value={tab} onChange={setTab} tabs={TABS} />
      <TabPanel id={tab} idPrefix="trk">
        {tab === 'card' ? (
          <div className="stack">
            <dl className="facts">
              {data.marking && (
                <>
                  <dt>
                    Marking
                    <InfoTip label="Marking">
                      The track&apos;s security label as a portion marking: its classification, restrictions and releasability (REL TO), taken
                      from the labels of the sources that report it (the highest classification, every restriction, only the countries every
                      source releases to). TAK events, NATS messages and exports carry the same marking. OpenTrack does not hide tracks by
                      clearance: everyone signed in sees every track, so handle each as its marking says.
                    </InfoTip>
                  </dt>
                  <dd className="mono">{data.marking}</dd>
                </>
              )}
              <dt>
                Class-name
                <InfoTip label="Class-name">
                  OTH-GOLD class and name, in capitals: the platform class (UNEQUATED when unknown) and the track&apos;s name or platform name
                  (UNKNOWN when unknown).
                </InfoTip>
              </dt>
              <dd>
                {m.class}-{m.name}
              </dd>
              <dt>
                Force code
                <InfoTip label="Force code">
                  The OTH-GOLD force code (Table 5-1, 0-39) for the track&apos;s domain and affiliation, e.g. 09 friendly surface, 07 hostile
                  surface, 32 unknown.
                </InfoTip>
              </dt>
              <dd className="mono">
                {String(m.force_code).padStart(2, '0')} · {m.domain} {m.affiliation}
              </dd>
              <dt>
                Track type
                <InfoTip label="Track type">
                  OTH-GOLD track type. tactical: a real-world track (the default). live_training: a real friendly track used for training.
                  simulated_training: made up for a training scenario. demand_entry: a real unit receivers must not filter out.
                </InfoTip>
              </dt>
              <dd className="mono">{m.track_type}</dd>
              <dt>
                SIDC
                <InfoTip label="SIDC">
                  The symbol code drawn for the track, with its standard (MIL-STD-2525C, 2525D or a CoT type). A feed&apos;s own code is kept,
                  with its affiliation set to the track&apos;s; otherwise a CoT type is built from the domain and affiliation.
                </InfoTip>
              </dt>
              <dd className="mono">
                {m.sidc.code} <span className="muted">({standardName(m.sidc.standard)})</span>
              </dd>
              <dt>Time</dt>
              <dd className="mono">{fmtTime(m.time)}</dd>
              <dt>Position</dt>
              <dd className="mono">
                {m.lat.toFixed(5)}, {m.lon.toFixed(5)}
              </dd>
              <dt>Course / speed</dt>
              <dd className="mono">
                {fmtNum(t.view.kinematics.course_deg, 0, '°')} / {fmtNum(t.view.kinematics.speed_mps, 1, ' m/s')}
              </dd>
              <dt>
                Subject
                <InfoTip label="Subject">The NATS subject this track is published on; consumers can subscribe to it alone.</InfoTip>
              </dt>
              <dd className="mono">{data.subject}</dd>
              {(t.bearings ?? []).length > 0 && (
                <>
                  <dt>
                    Bearings
                    <InfoTip label="Bearings">
                      Lines of bearing that point at this track, the latest from each sensor: the source, the bearing it measured ± its
                      1σ error, and how far that line misses the track&apos;s position, in degrees. Hover a line for its time.
                    </InfoTip>
                  </dt>
                  <dd className="stack" style={{ gap: 2 }}>
                    {(t.bearings ?? []).map((b) => (
                      <span key={`${b.source_id}/${b.source_track_key}`} className="mono" title={`at ${fmtTime(b.observed_at)}`}>
                        {b.source_id} {b.bearing_deg.toFixed(1)}° ±{b.sigma_deg.toFixed(1)}, misses by {Math.abs(b.residual_deg).toFixed(1)}°
                        {b.range_m !== undefined && ` · range ${b.range_m.toFixed(0)} m${b.range_sigma_m !== undefined ? ` ±${b.range_sigma_m.toFixed(0)} m` : ''}`}
                        {(b.identifiers ?? []).map((i) => ` · ${i.scheme} ${i.value}`).join('')}
                      </span>
                    ))}
                  </dd>
                </>
              )}
              {t.reported_by && (
                <>
                  <dt>Reported by</dt>
                  <dd className="num-row">
                    <span className="mono">{t.reported_by}</span>
                    <InfoTip label="Reported by">
                      With other OpenTrack nodes sharing the picture, the node that sees this track best reports it to the others; the rest hold it
                      quietly and take over if it stops.
                    </InfoTip>
                  </dd>
                </>
              )}
              <dt>
                Entity
                <InfoTip label="Entity">
                  The registry entity this track resolves to, once a source&apos;s identifiers match it well enough. For the fields a source&apos;s
                  pipeline links to the entity, the entity&apos;s values are published in place of what the feed reports.
                </InfoTip>
              </dt>
              <dd className="mono">{entityId ?? <span className="muted">none</span>}</dd>
              {(t.members ?? []).length > 0 && (
                <>
                  <dt>Members</dt>
                  <dd className="mono">{(t.members ?? []).map((u) => `tms-${u}`).join(', ')}</dd>
                </>
              )}
              {(t.groups ?? []).length > 0 && (
                <>
                  <dt>Groups</dt>
                  <dd className="mono">{(t.groups ?? []).join(', ')}</dd>
                </>
              )}
              {(t.paired_with ?? []).length > 0 && (
                <>
                  <dt>Paired with</dt>
                  <dd>
                    {(t.paired_with ?? []).map((p) => (
                      <span key={p} className="num-row">
                        <span className="mono">tms-{p}</span>
                        <Button
                          size="xs"
                          variant="ghost"
                          icon={<TbUnlink />}
                          aria-label={`Unpair tms-${p}`}
                          disabled={!canManage}
                          title="Unpair"
                          onClick={async () => {
                            try {
                              await api.unpairTracks(m.track_id, `tms-${p}`)
                              toast({ variant: 'success', title: 'Unpaired', message: `tms-${p}` })
                            } catch (e) {
                              toast({ variant: 'error', title: 'Not unpaired', message: errorMessage(e) })
                            }
                          }}
                        />
                      </span>
                    ))}
                  </dd>
                </>
              )}
            </dl>
            {(t.notices ?? []).length > 0 && (
              <div className="stack" style={{ gap: 4 }}>
                {(t.notices ?? []).map((n) => (
                  <span key={`${n.key}:${n.source_id}`} className="notice">
                    <TbAlertTriangle aria-hidden /> <span className="mono">{n.key}</span>: {n.source_id} reports{' '}
                    <span className="mono">{show(n.feed)}</span>; the entity&apos;s <span className="mono">{show(n.entity)}</span> is published.
                  </span>
                ))}
              </div>
            )}
            <h3 className="subhead">
              Attributes
              <InfoTip label="Attributes">
                The published output schema&apos;s fields and this track&apos;s values. Beside a value: OpenTrack means a built-in value it
                computes; entity means the entity replaced what a feed reports; nothing means it came from the feeds.
              </InfoTip>
            </h3>
            {fields.length === 0 ? (
              <span className="muted">The output schema has no attributes. Add fields on the Schema page.</span>
            ) : (
              <dl className="facts">
                {fields.map((f) => (
                  <div key={f.key} style={{ display: 'contents' }}>
                    <dt className="mono">{f.key}</dt>
                    <dd>
                      {attrs[f.key] === undefined ? (
                        <span className="muted">—</span>
                      ) : (
                        <>
                          <span className="mono">{show(attrs[f.key])}</span>{' '}
                          <span className="muted">{origin(f, (notices[`ext.${f.key}`] ?? []).length > 0)}</span>
                        </>
                      )}
                    </dd>
                  </div>
                ))}
              </dl>
            )}
          </div>
        ) : tab === 'history' ? (
          <TrackHistory
            uid={t.uid}
            points={history}
            onMap={historyOnMap}
            onToggleMap={onHistoryOnMap}
            onZoom={onZoom}
            onChanged={() => {
              setHistoryRev((n) => n + 1)
              onChanged?.()
            }}
          />
        ) : (
          <div className="stack">
            <h3 className="subhead">
              Source tracks
              {data.confidence != null && (
                <span className="muted">
                  {' '}
                  · confidence {pct(data.confidence)}
                </span>
              )}
              <InfoTip label="Source tracks">
                Each source track reporting for this track, as source/key. Paired by: how it joined (a shared identifier, agreeing
                kinematics, or it started the track). Confidence: the chance it is the same object as the rest; the second figure, when a
                source gives one, is the chance it is a real object at all. The header&apos;s confidence is the chance the track is real: at
                least one source track is a real report of it. Split sends a source track off to a track of its own and keeps the two
                apart.
              </InfoTip>
            </h3>
            <DataTable
              aria-label="Source tracks"
              columns={contributorColumns}
              rows={t.contributors}
              rowKey={(c) => `${c.source_id}/${c.source_track_key}`}
              empty="No source track reports for this track."
            />
            <h3 className="subhead">
              Lineage
              <InfoTip label="Lineage">The graph of source tracks, merges, groups, identifiers and entities behind this track, over time.</InfoTip>
            </h3>
            {edges === null ? (
              <span className="muted">LOADING…</span>
            ) : edges.length === 0 ? (
              <span className="muted">No graph history.</span>
            ) : (
              <>
                <Suspense fallback={<span className="muted">LOADING…</span>}>
                  <LineageGraph uid={t.uid} edges={edges} entityId={t.entity_id} />
                </Suspense>
                <h3 className="subhead">
                  Correlation decisions
                  <InfoTip label="Correlation decisions">
                    Every link in the lineage, blue while it holds, grey once ended. REPORTS_FOR: a source track reports for a track.
                    MERGED_INTO: a track merged into another. CANDIDATE_OF: a source track scored as a match. DO_NOT_PAIR: two source tracks
                    kept apart after a split or a rejected suggestion. CARRIES: a source track carries an identifier. RESOLVES_TO: an
                    identifier names an entity. MEMBER_OF: a track in a group. PAIRED_WITH: two tracks paired by an operator. Each names the
                    decision behind it; an operator&apos;s can be undone.
                  </InfoTip>
                </h3>
                <ul className="history">
                  {edges.map((e) => (
                    <li key={e.id} className={e.valid_to_ms === null ? '' : 'ended'}>
                      <Badge color={e.valid_to_ms === null ? 'blue' : 'grey'} size="sm">
                        {e.kind}
                      </Badge>
                      <span className="mono">
                        {e.src_key} → {e.dst_key}
                      </span>
                      <span />
                      <span className="muted">
                        {fmtTime(e.valid_from_ms)}
                        {e.valid_to_ms !== null && ` – ${fmtTime(e.valid_to_ms)}`} · decision #{e.decision_id} (
                        {e.decision_op} by {e.decision_actor})
                      {e.valid_to_ms === null && undoable({ op: e.decision_op, actor: e.decision_actor, undone_by: undefined }) && (
                        <Button
                          size="xs"
                          variant="ghost"
                          icon={<TbArrowBackUp />}
                          title={`Undo ${opLabel(e.decision_op)} (#${e.decision_id})`}
                          aria-label={`Undo decision ${e.decision_id}`}
                          disabled={!canManage}
                          onClick={async () => {
                            if (await undo({ id: e.decision_id, op: e.decision_op })) {
                              setEdgesRev((n) => n + 1)
                              onChanged?.()
                            }
                          }}
                        />
                      )}
                      </span>
                    </li>
                  ))}
                </ul>
              </>
            )}
            <dl className="facts">
              <dt>First seen</dt>
              <dd className="mono">{fmtTime(t.first_seen)}</dd>
              <dt>Last seen</dt>
              <dd className="mono">{fmtTime(t.last_seen)}</dd>
              <dt>
                Observations
                <InfoTip label="Observations">Reports applied to this track since it started, from all its sources.</InfoTip>
              </dt>
              <dd className="mono">{t.observation_count.toLocaleString()}</dd>
            </dl>
          </div>
        )}
      </TabPanel>
    </div>
  )
}
