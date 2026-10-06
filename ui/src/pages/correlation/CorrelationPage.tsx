import { useCallback, useEffect, useState } from 'react'
import { TbCheck, TbRestore, TbX } from 'react-icons/tb'
import { Badge, Button, CollapsiblePanel, DataTable, FieldSelect, SaveButton, useToast, type DataTableColumn } from '@kineticlogic/staresdk'
import { SUGGESTIONS_CHANGED } from '../../lib/pendingSuggestions'
import { api, type CorrelationSettings, type DecisionRow, type Suggestion, type SuggestionTrack } from '../../api/client'
import { errorMessage, fmtTime } from '../../lib/format'
import { SettingsForm } from './SettingsForm'
import { InfoTip } from '../../components/InfoTip'
import { useCan } from '../../auth/context'
import { FILL_PANEL, usePanelOpen } from '../../lib/panelOpen'

const REFRESH_MS = 5000
const STATUSES = [{ name: 'open' }, { name: 'accepted' }, { name: 'rejected' }, { name: 'expired' }, { name: 'all' }]

function TrackLink({ t }: { t: SuggestionTrack | null }) {
  if (!t) return null
  const label = t.name ?? t.track_id
  return t.live ? (
    <a href={`#tracks/${encodeURIComponent(t.uid)}`} title={`${t.track_id}${t.published === false ? ' (not published)' : ''}`}>
      {label}
    </a>
  ) : (
    <span className="muted" title="No longer live">
      {label}
    </span>
  )
}

const OP_COLOR: Record<string, 'blue' | 'success' | 'warning' | 'danger' | 'grey'> = {
  pair: 'success',
  merge: 'success',
  split: 'warning',
  do_not_pair: 'danger',
  reject_split: 'grey',
  correlation_settings: 'blue',
}

// A closed panel in the top row keeps its title's height instead of stretching to its neighbour.
const CLOSED_IN_ROW = { alignSelf: 'start' } as const

/**
 * Correlation: the engine's pairing and split suggestions for an operator to accept or reject,
 * the settings it correlates with, and the decisions it and operators have made.
 */
export default function CorrelationPage() {
  const { toast } = useToast()
  const canManage = useCan('track_manager')
  const canAdmin = useCan('admin')
  const [status, setStatus] = useState('open')
  const [suggestions, setSuggestions] = useState<Suggestion[] | null>(null)
  const [busy, setBusy] = useState<number | null>(null)
  const [settings, setSettings] = useState<CorrelationSettings | null>(null)
  const [draft, setDraft] = useState<CorrelationSettings | null>(null)
  const [defaults, setDefaults] = useState<CorrelationSettings | null>(null)
  const [version, setVersion] = useState('')
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)
  const [decisions, setDecisions] = useState<DecisionRow[] | null>(null)
  const [picked, setPicked] = useState<number | null>(null)
  const [suggestionsOpen, setSuggestionsOpen] = usePanelOpen('ot.panel.suggestions')
  const [settingsOpen, setSettingsOpen] = usePanelOpen('ot.panel.correlationSettings')
  const [decisionsOpen, setDecisionsOpen] = usePanelOpen('ot.panel.correlationDecisions')

  const loadSuggestions = useCallback(
    () => api.suggestions(status).then((r) => setSuggestions(r.suggestions), (e) => toast({ variant: 'error', title: 'Suggestions', message: errorMessage(e) })),
    [status, toast],
  )
  const loadDecisions = useCallback(() => api.correlationDecisions(200).then((r) => setDecisions(r.decisions), () => setDecisions([])), [])

  useEffect(() => {
    loadSuggestions()
    loadDecisions()
    const t = setInterval(() => {
      loadSuggestions()
      loadDecisions()
    }, REFRESH_MS)
    return () => clearInterval(t)
  }, [loadSuggestions, loadDecisions])

  useEffect(() => {
    api.correlationSettings().then(
      (r) => {
        setSettings(r.settings)
        setDraft(r.settings)
        setDefaults(r.defaults)
        setVersion(r.version)
      },
      (e) => toast({ variant: 'error', title: 'Correlation settings', message: errorMessage(e) }),
    )
  }, [toast])

  const decide = async (s: Suggestion, decision: 'accept' | 'reject') => {
    setBusy(s.id)
    try {
      await api.decideSuggestion(s.id, decision)
      // The tab bar's pending count follows at once.
      window.dispatchEvent(new Event(SUGGESTIONS_CHANGED))
      toast({ variant: 'success', title: decision === 'accept' ? 'Accepted' : 'Rejected', message: `Suggestion ${s.id}` })
    } catch (e) {
      toast({ variant: 'error', title: `Could not ${decision}`, message: errorMessage(e) })
    } finally {
      setBusy(null)
      loadSuggestions()
      loadDecisions()
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
      loadDecisions()
    }
  }

  const columns: DataTableColumn<Suggestion>[] = [
    {
      key: 'kind',
      header: 'Proposal',
      width: 80,
      render: (s) => (
        <Badge color={s.kind === 'pair' ? 'blue' : 'warning'} size="sm" uppercase>
          {s.kind}
        </Badge>
      ),
      sortValue: (s) => s.kind,
    },
    {
      key: 'what',
      header: 'Tracks',
      render: (s) =>
        s.kind === 'pair' ? (
          <span>
            <TrackLink t={s.b} /> <span className="muted">into</span> <TrackLink t={s.a} />
          </span>
        ) : (
          <span>
            <span className="mono">{s.source_track}</span> <span className="muted">off</span> <TrackLink t={s.a} />
          </span>
        ),
    },
    {
      key: 'why',
      header: 'Evidence',
      render: (s) => (
        <span className="muted" title={s.evidence.reason}>
          {s.evidence.reason ?? '—'}
        </span>
      ),
    },
    { key: 'updated', header: 'Updated', width: 170, mono: true, render: (s) => fmtTime(s.updated_at_ms), sortValue: (s) => s.updated_at_ms },
    {
      key: 'act',
      header: '',
      width: 190,
      align: 'right',
      render: (s) =>
        s.status === 'open' ? (
          <span className="value-row" style={{ justifyContent: 'flex-end', flexWrap: 'nowrap' }}>
            <Button size="sm" variant="primary" icon={<TbCheck />} disabled={!canManage || busy !== null} onClick={() => decide(s, 'accept')}>
              Accept
            </Button>
            <Button size="sm" variant="secondary" icon={<TbX />} disabled={!canManage || busy !== null} onClick={() => decide(s, 'reject')}>
              Reject
            </Button>
          </span>
        ) : (
          <Badge color="grey" size="sm" uppercase>
            {s.status}
          </Badge>
        ),
    },
  ]

  const decisionColumns: DataTableColumn<DecisionRow>[] = [
    { key: 'at', header: 'Time', width: 170, mono: true, render: (d) => fmtTime(d.at_ms), sortValue: (d) => d.at_ms },
    {
      key: 'op',
      header: 'Decision',
      width: 170,
      render: (d) => (
        <Badge color={OP_COLOR[d.op] ?? 'grey'} size="sm">
          {d.op}
        </Badge>
      ),
      sortValue: (d) => d.op,
    },
    { key: 'actor', header: 'By', width: 110, render: (d) => d.actor, sortValue: (d) => d.actor },
    { key: 'reason', header: 'Reason', render: (d) => <span title={d.reason ?? ''}>{d.reason ?? '—'}</span> },
  ]
  const pickedDecision = decisions?.find((d) => d.id === picked) ?? null
  const dirty = draft !== null && JSON.stringify(draft) !== JSON.stringify(settings)
  const open = suggestions?.filter((s) => s.status === 'open').length ?? 0

  return (
    <div className="panels tight fill-page">
      <div className="correlation-top">
        <CollapsiblePanel
          title="Suggestions"
          badge={status === 'open' && open ? String(open) : undefined}
          open={suggestionsOpen}
          onOpenChange={setSuggestionsOpen}
          style={suggestionsOpen ? undefined : CLOSED_IN_ROW}
          titleActions={
            <div className="title-tools">
              <FieldSelect ariaLabel="Suggestion status" fields={STATUSES} value={status} onChange={(v) => setStatus(v ?? 'open')} style={{ width: 120 }} />
              <InfoTip label="Suggestions">
                What the engine proposes and waits on an operator for. PAIR: in suggest mode, two tracks whose motion agrees; accept pairs them, reject records them as different objects so they are not proposed again. SPLIT: a source
                track stopped agreeing with its track; accept splits it off, reject keeps it there and silences the proposal for 30 minutes.
                Evidence is the engine&apos;s reason. The filter shows open, accepted, rejected or expired suggestions; expired ones concerned a
                track that no longer exists.
              </InfoTip>
            </div>
          }
        >
          <div className="panel-body fill">
            <DataTable
              aria-label="Suggestions"
              columns={columns}
              rows={suggestions ?? []}
              rowKey={(s) => String(s.id)}
              maxHeight="none"
              empty={
                suggestions === null
                  ? 'LOADING…'
                  : status === 'open'
                    ? 'Nothing to decide. Pairings wait here in suggest mode, and splits when a source track stops agreeing with its track.'
                    : `No ${status === 'all' ? '' : status + ' '}suggestions.`
              }
            />
          </div>
        </CollapsiblePanel>
        <CollapsiblePanel
          title="Settings"
          open={settingsOpen}
          onOpenChange={setSettingsOpen}
          style={settingsOpen ? undefined : CLOSED_IN_ROW}
          titleActions={
            <div className="title-tools">
              {version && <span className="mono muted">{version}</span>}
              <InfoTip label="Correlation settings">
                How the engine pairs, proposes and splits tracks, and what it publishes; the code is the engine version. Saved settings apply
                within seconds and are recorded as a decision. Defaults fills the form with the built-in values; nothing changes until you save.
              </InfoTip>
            </div>
          }
          actions={
            <>
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
            </>
          }
        >
          <div className="panel-body correlation-settings">{draft ? <SettingsForm value={draft} onChange={setDraft} /> : <span className="muted">LOADING…</span>}</div>
        </CollapsiblePanel>
      </div>
      <CollapsiblePanel
        title="Decisions"
        open={decisionsOpen}
        onOpenChange={setDecisionsOpen}
        style={decisionsOpen ? FILL_PANEL : undefined}
        titleActions={
          <InfoTip label="Decisions">
            The latest 200 correlation decisions, by the engine or an operator; click one for its evidence. pair: a source track joined a
            track. merge: two tracks became one. split: a source track left its track. do_not_pair: tracks recorded as different objects.
            reject_split: a proposed split refused. end_source_track: its source ended it. retire_system_track: every source track of a
            track ended. correlation_settings: the settings were saved.
          </InfoTip>
        }
      >
        <div className="panel-body fill">
          <DataTable
            aria-label="Correlation decisions"
            columns={decisionColumns}
            rows={decisions ?? []}
            rowKey={(d) => String(d.id)}
            selectedKey={picked === null ? null : String(picked)}
            onRowClick={(d) => setPicked(d.id === picked ? null : d.id)}
            maxHeight="none"
            empty={decisions === null ? 'LOADING…' : 'No correlation decisions yet.'}
          />
          {pickedDecision && (
            <pre className="mono evidence" aria-label={`Evidence of decision ${pickedDecision.id}`}>
              {JSON.stringify(pickedDecision.evidence ?? {}, null, 2)}
            </pre>
          )}
        </div>
      </CollapsiblePanel>
    </div>
  )
}
