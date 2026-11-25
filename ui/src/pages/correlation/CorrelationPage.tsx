import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react'
import { TbRestore } from 'react-icons/tb'
import { Badge, Button, DataTable, EmptyState, FieldSelect, Modal, SaveButton, TabPanel, Tabs, useToast, type DataTableColumn } from '@kineticlogic/staresdk'
import { SUGGESTIONS_CHANGED } from '../../lib/pendingSuggestions'
import { api, type CorrelationSettings, type DecisionRow, type Suggestion, type SuggestionTrack } from '../../api/client'
import { errorMessage, fmtTime } from '../../lib/format'
import { SettingsForm } from './SettingsForm'
import { InfoTip } from '../../components/InfoTip'
import { useCan } from '../../auth/context'
import { DetailDrawer } from '../../lib/DetailDrawer'
import { DecisionDetails, SuggestionDetails, trackName } from './CorrelationDetails'
import { evidenceTracks } from '../trackdb/undo'
import { decisionCase, suggestionCase, TuningCase, type CorrelationCase, type TuningTarget } from './TuningCase'
import { TuningGuide } from './TuningGuide'

const REFRESH_MS = 5000
const TrackCard = lazy(() => import('../trackdb/TrackCard').then((module) => ({ default: module.TrackCard })))
const BACKGROUND_TABLE = { border: 0, borderRadius: 0 } as const
const STATUSES = [{ name: 'open' }, { name: 'accepted' }, { name: 'rejected' }, { name: 'expired' }, { name: 'all' }]
const VIEWS = [
  { id: 'suggestions', label: 'Suggestions' },
  { id: 'decisions', label: 'Decisions' },
  { id: 'settings', label: 'Settings' },
]

const OP_COLOR: Record<string, 'blue' | 'success' | 'warning' | 'danger' | 'grey'> = {
  pair: 'success',
  merge: 'success',
  split: 'warning',
  do_not_pair: 'danger',
  reject_split: 'grey',
  correlation_settings: 'blue',
}

function usePolledRows<T>(read: () => Promise<T[]>, revision: number) {
  const [result, setResult] = useState<{ read: typeof read; rows: T[] | null; error: string | null } | null>(null)
  useEffect(() => {
    let live = true
    const load = () => read().then(
      (rows) => { if (live) setResult({ read, rows, error: null }) },
      (error) => {
        if (live) setResult((current) => ({
          read,
          rows: current?.read === read ? current.rows : null,
          error: errorMessage(error),
        }))
      },
    )
    void load()
    const timer = setInterval(load, REFRESH_MS)
    return () => {
      live = false
      clearInterval(timer)
    }
  }, [read, revision])
  return result?.read === read ? result : { rows: null, error: null }
}

function engineMode(settings: CorrelationSettings): string {
  const pairing = settings.approach === 'identifiers'
    ? 'OpenTrack pairs tracks automatically when a shared ID passes its checks.'
    : settings.mode === 'suggest'
      ? 'You review matches based on movement. Matches based on a shared ID can happen automatically.'
      : 'OpenTrack pairs tracks automatically.'
  const splitting = settings.split.automatic
    ? 'OpenTrack applies splits automatically.'
    : settings.split.propose
      ? 'You review proposed splits.'
      : 'Splits are off.'
  return `${pairing} ${splitting}`
}

const suggestionColumns: DataTableColumn<Suggestion>[] = [
  {
    key: 'kind',
    header: 'Proposal',
    width: 80,
    render: (s) => <Badge color={s.kind === 'pair' ? 'blue' : 'warning'} size="sm" uppercase>{s.kind}</Badge>,
    sortValue: (s) => s.kind,
  },
  {
    key: 'what',
    header: 'Tracks',
    width: 200,
    render: (s) => s.kind === 'pair' ? (
      <span>{trackName(s.b)} <span className="muted">into</span> {trackName(s.a)}</span>
    ) : (
      <span><span className="mono">{s.source_track}</span> <span className="muted">off</span> {trackName(s.a)}</span>
    ),
  },
  {
    key: 'why',
    header: 'Evidence',
    width: 270,
    render: (s) => <span className="muted" title={s.evidence.reason}>{s.evidence.reason ?? '—'}</span>,
  },
  { key: 'updated', header: 'Updated', width: 170, mono: true, render: (s) => fmtTime(s.updated_at_ms), sortValue: (s) => s.updated_at_ms },
  {
    key: 'act',
    header: '',
    width: 96,
    align: 'right',
    render: (s) => s.status !== 'open' && <Badge color="grey" size="sm" uppercase>{s.status}</Badge>,
  },
]

const decisionColumns: DataTableColumn<DecisionRow>[] = [
  { key: 'at', header: 'Time', width: 170, mono: true, render: (d) => fmtTime(d.at_ms), sortValue: (d) => d.at_ms },
  {
    key: 'op',
    header: 'Decision',
    width: 190,
    render: (d) => (
      <div className="correlation-decision">
        <Badge color={OP_COLOR[d.op] ?? 'grey'} size="sm">{d.op}</Badge>
        {d.undone_by != null && <span className="muted">undone by #{d.undone_by}</span>}
      </div>
    ),
    sortValue: (d) => d.op,
  },
  { key: 'actor', header: 'By', width: 110, render: (d) => d.actor, sortValue: (d) => d.actor },
  { key: 'tracks', header: 'Tracks', width: 190, render: (d) => {
    const tracks = evidenceTracks(d.evidence).join(', ')
    return <span className="mono" title={tracks}>{tracks || '—'}</span>
  } },
  { key: 'reason', header: 'Reason', width: 240, render: (d) => <span title={d.reason ?? ''}>{d.reason ?? '—'}</span> },
]

/** Review suggestions, inspect decisions, and change the engine's settings. */
export default function CorrelationPage() {
  const { toast } = useToast()
  const canManage = useCan('track_manager')
  const canAdmin = useCan('admin')
  const [view, setView] = useState('suggestions')
  const [status, setStatus] = useState('open')
  const [revision, setRevision] = useState(0)
  const [busy, setBusy] = useState<number | null>(null)
  const [settings, setSettings] = useState<CorrelationSettings | null>(null)
  const [draft, setDraft] = useState<CorrelationSettings | null>(null)
  const [defaults, setDefaults] = useState<CorrelationSettings | null>(null)
  const [version, setVersion] = useState('')
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)
  const [picked, setPicked] = useState<number | null>(null)
  const [tuningCase, setTuningCase] = useState<CorrelationCase | null>(null)
  const [shortcut, setShortcut] = useState<{ target: TuningTarget } | null>(null)
  const [inspectedTrack, setInspectedTrack] = useState<SuggestionTrack | null>(null)
  const trackTrigger = useRef<HTMLButtonElement | null>(null)
  const focusTrackCard = useCallback((content: HTMLDivElement | null) => content?.focus({ preventScroll: true }), [])
  const closeTrackCard = () => {
    const trigger = trackTrigger.current
    setInspectedTrack(null)
    trackTrigger.current = null
    requestAnimationFrame(() => trigger?.focus({ preventScroll: true }))
  }

  const readSuggestions = useCallback(() => api.suggestions(status).then((r) => r.suggestions), [status])
  const readDecisions = useCallback(() => api.correlationDecisions(200).then((r) => r.decisions), [])
  const { rows: suggestions, error: suggestionError } = usePolledRows(readSuggestions, revision)
  const { rows: decisions, error: decisionError } = usePolledRows(readDecisions, revision)

  useEffect(() => {
    let live = true
    api.correlationSettings().then(
      (r) => {
        if (!live) return
        setSettings(r.settings)
        setDraft(r.settings)
        setDefaults(r.defaults)
        setVersion(r.version)
      },
      (e) => { if (live) toast({ variant: 'error', title: 'Correlation settings', message: errorMessage(e) }) },
    )
    return () => { live = false }
  }, [toast])

  const decide = async (s: Suggestion, decision: 'accept' | 'reject') => {
    setBusy(s.id)
    try {
      await api.decideSuggestion(s.id, decision)
      setPicked((current) => current === s.id ? null : current)
      window.dispatchEvent(new Event(SUGGESTIONS_CHANGED))
      toast({ variant: 'success', title: decision === 'accept' ? 'Accepted' : 'Rejected', message: `Suggestion ${s.id}` })
    } catch (e) {
      toast({ variant: 'error', title: `Could not ${decision}`, message: errorMessage(e) })
    } finally {
      setBusy(null)
      setRevision((n) => n + 1)
    }
  }

  const save = async () => {
    if (!draft) return
    setSaving(true)
    try {
      const r = await api.saveCorrelationSettings(draft)
      setSettings(r.settings)
      setDraft(r.settings)
      setSaved(true)
      setTimeout(() => setSaved(false), 1500)
    } catch (e) {
      toast({ variant: 'error', title: 'Settings not saved', message: errorMessage(e) })
    } finally {
      setSaving(false)
      setRevision((n) => n + 1)
    }
  }

  const pickedSuggestion = view === 'suggestions' ? suggestions?.find((s) => s.id === picked) : undefined
  const pickedDecision = view === 'decisions' ? decisions?.find((d) => d.id === picked) : undefined
  const pickedDecisionCase = pickedDecision && decisionCase(pickedDecision)
  const openTuningCase = (example: CorrelationCase) => {
    setTuningCase(example)
    setShortcut(null)
    setPicked(null)
    setView('settings')
  }
  const dirty = draft !== null && JSON.stringify(draft) !== JSON.stringify(settings)
  const open = suggestions?.filter((s) => s.status === 'open').length ?? 0

  return (
    <>
      <div className="correlation-workspace" inert={!!inspectedTrack}>
        <Tabs
          aria-label="Correlation"
          idPrefix="correlation"
          value={view}
          onChange={(value) => { setView(value); setPicked(null); setShortcut(null) }}
          tabs={VIEWS.map((tab) => tab.id === 'suggestions' ? { ...tab, badge: open } : tab)}
        />
        <TabPanel id={view} idPrefix="correlation" style={{ flex: 1, display: 'flex', flexDirection: 'column' }}>
          {view === 'suggestions' && (
            <div className="correlation-view">
              <div className="toolbar correlation-toolbar">
                <FieldSelect
                  ariaLabel="Suggestion status"
                  fields={STATUSES}
                  value={status}
                  onChange={(value) => { setStatus(value ?? 'open'); setPicked(null) }}
                  style={{ width: 120 }}
                />
                <InfoTip label="Suggestions">
                  {settings && <><div>{engineMode(settings)}</div><br /></>}
                  A pair would combine two tracks. A split would move one source&apos;s reports to their own track.
                  Open a row to see why OpenTrack suggested the change. Use the filter to find suggestions waiting for review,
                  ones you&apos;ve already reviewed, or ones that expired because the tracks changed or ended.
                </InfoTip>
              </div>
              {suggestionError && <div className="error-text" role="alert">{suggestionError}</div>}
              <DataTable
                aria-label="Suggestions"
                columns={suggestionColumns}
                rows={suggestions ?? []}
                rowKey={(s) => String(s.id)}
                selectedKey={picked === null ? null : String(picked)}
                onRowClick={(s) => setPicked(s.id)}
                pinFirstColumn
                maxHeight={420}
                style={BACKGROUND_TABLE}
                empty={suggestionError ? null : suggestions === null ? 'LOADING…' : status === 'open'
                  ? <EmptyState title="Nothing to decide." />
                  : `No ${status === 'all' ? '' : status + ' '}suggestions.`}
              />
              <DetailDrawer open={!!pickedSuggestion} title="Suggestions" label="Suggestions" width={530} inline onClose={() => setPicked(null)}>
                {pickedSuggestion && (
                  <SuggestionDetails
                    key={pickedSuggestion.id}
                    suggestion={pickedSuggestion}
                    canManage={canManage}
                    busy={busy !== null}
                    onTune={() => openTuningCase(suggestionCase(pickedSuggestion))}
                    onDecide={(decision) => void decide(pickedSuggestion, decision)}
                    onInspect={(track, trigger) => {
                      trackTrigger.current = trigger
                      setInspectedTrack(track)
                    }}
                  />
                )}
              </DetailDrawer>
            </div>
          )}
          {view === 'decisions' && (
            <div className="correlation-view">
              <div className="toolbar correlation-toolbar">
                <InfoTip label="Decisions">
                  This log shows the last 200 decisions made by OpenTrack or an operator.
                  Open a row to see what happened, who made the decision, and what OpenTrack recorded about it.
                </InfoTip>
              </div>
              {decisionError && <div className="error-text" role="alert">{decisionError}</div>}
              <DataTable
                aria-label="Correlation decisions"
                columns={decisionColumns}
                rows={decisions ?? []}
                rowKey={(d) => String(d.id)}
                selectedKey={picked === null ? null : String(picked)}
                onRowClick={(d) => setPicked(d.id)}
                pinFirstColumn
                maxHeight={420}
                style={BACKGROUND_TABLE}
                empty={decisionError ? null : decisions === null ? 'LOADING…' : <EmptyState title="No correlation decisions yet." />}
              />
              <DetailDrawer open={!!pickedDecision} title="Decisions" label="Decisions" width={530} inline onClose={() => setPicked(null)}>
                {pickedDecision && (
                  <DecisionDetails
                    key={pickedDecision.id}
                    decision={pickedDecision}
                    onTune={pickedDecisionCase ? () => openTuningCase(pickedDecisionCase) : undefined}
                  />
                )}
              </DetailDrawer>
            </div>
          )}
          {view === 'settings' && (
            <div className="panels">
              <div className="toolbar correlation-toolbar">
                {version && <span className="mono muted">{version}</span>}
                <InfoTip label="Correlation settings">
                  Choose how OpenTrack pairs and splits tracks, what you review, and which tracks it sends to other systems.
                  The code shows the version of the matching software. Defaults fills in the original values.
                  Select Save to apply your changes. They take effect within seconds and appear in Decisions.
                </InfoTip>
                <span className="spacer" />
                {draft && <TuningGuide draft={draft} onSelect={(target) => setShortcut({ target })} />}
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<TbRestore />}
                  disabled={!canAdmin || !defaults || JSON.stringify(draft) === JSON.stringify(defaults)}
                  onClick={() => defaults && setDraft(defaults)}
                >
                  Defaults
                </Button>
                {canAdmin && <SaveButton size="sm" dirty={dirty} saving={saving} saved={saved} onSave={save} />}
              </div>
              {draft ? (
                <>
                  {tuningCase && <TuningCase example={tuningCase} draft={draft} onClear={() => setTuningCase(null)} />}
                  <SettingsForm value={draft} onChange={setDraft} caseTarget={tuningCase?.target} shortcut={shortcut} />
                </>
              ) : <span className="muted">LOADING…</span>}
            </div>
          )}
        </TabPanel>
      </div>
      {inspectedTrack && (
        <Modal title="Track card" width={640} onClose={closeTrackCard}>
          <div ref={focusTrackCard} tabIndex={-1}>
            <Suspense fallback={<div className="panel-body muted">LOADING…</div>}>
              <TrackCard key={inspectedTrack.uid} uid={inspectedTrack.uid} onChanged={() => setRevision((n) => n + 1)} />
            </Suspense>
          </div>
        </Modal>
      )}
    </>
  )
}
