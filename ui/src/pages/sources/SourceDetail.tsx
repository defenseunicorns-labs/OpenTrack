import { useEffect, useMemo, useState } from 'react'
import { TbTrash } from 'react-icons/tb'
import { Badge, Button, DataTable, SaveButton, TabPanel, Tabs, Toggle, useToast, type DataTableColumn } from '@kineticlogic/staresdk'
import { CodeEditor } from '@kineticlogic/staresdk/code-editor'
import { api, type MetricsResponse, type SourceRow, type SourceSpec } from '../../api/client'
import { ago, errorMessage, fmtCount, fmtNum, fmtTime } from '../../lib/format'
import { DetailDrawer } from '../../lib/DetailDrawer'
import { PipelineView } from './PipelineView'
import { PIPELINE_COUNTS_INFO, SOURCE_STATES_INFO, sourceState } from '../../lib/sourceState'
import { SenderAuth } from './SenderAuth'
import { TransportForm } from './TransportForm'
import { useCan } from '../../auth/context'
import { InfoTip } from '../../components/InfoTip'

type Revision = Awaited<ReturnType<typeof api.revisions>>[number]

const TABS = [
  { id: 'status', label: 'Status' },
  { id: 'transport', label: 'Transport' },
  { id: 'pipeline', label: 'Pipeline' },
  { id: 'history', label: 'History' },
]

const REVISION_COLUMNS: DataTableColumn<Revision>[] = [
  { key: 'rev', header: 'Revision', width: 80, render: (r) => r.revision, sortValue: (r) => r.revision },
  { key: 'at', header: 'Saved', mono: true, render: (r) => fmtTime(r.saved_at_ms) },
  { key: 'decision', header: 'Decision', width: 90, align: 'right', render: (r) => (r.decision_id ? `#${r.decision_id}` : '—') },
]

function StatusTab({ source }: { source: SourceRow }) {
  const [metrics, setMetrics] = useState<MetricsResponse | null>(null)
  useEffect(() => {
    api.metrics(source.id, 60).then(setMetrics, () => setMetrics(null))
  }, [source.id, source.status?.updated_at])
  const link = source.status?.link
  const timing = source.status?.tracker_timing
  const totals = Object.entries(metrics?.totals ?? {}).filter(([k]) => !k.includes(':'))
  return (
    <div className="stack" style={{ gap: 8 }}>
      <dl className="facts">
        <dt>Transport</dt>
        <dd className="mono">
          {source.transport} · {String(source.spec.transport.url ?? source.spec.transport.bind ?? `${source.spec.transport.host ?? ''}:${source.spec.transport.port ?? ''}`)}
        </dd>
        <dt>Codec</dt>
        <dd className="mono">{source.codec}</dd>
        <dt>Link</dt>
        <dd>
          {link ? `${link.connected ? 'connected' : 'not connected'} · ${link.connects} connect(s) · ${link.errors} error(s)` : 'no worker running'}
        </dd>
        <dt>Last frame</dt>
        <dd>{link?.last_frame_at ? `${ago(link.last_frame_at)} ago` : '—'}</dd>
        <dt>Revision</dt>
        <dd>
          {source.revision} · saved {fmtTime(source.updated_at_ms)}
        </dd>
        {(source.spec.pipeline as { tracker?: { auto_timing?: unknown } }).tracker?.auto_timing != null && (
          <>
            <dt>Tracker timing</dt>
            <dd>
              {timing
                ? `revisit ${fmtNum(timing.revisit_secs, 1)} s (${timing.revisit_source}) · confirm within ${fmtNum(timing.confirm_within_secs, 0)} s · drop after ${fmtNum(timing.drop_tentative_secs, 0)} s unconfirmed, ${fmtNum(timing.drop_confirmed_secs, 0)} s confirmed`
                : 'waiting for the sensor’s revisit rate'}
            </dd>
          </>
        )}
        <dt>Raw output</dt>
        <dd className="mono">{source.raw_subject ?? 'off'}</dd>
      </dl>
      {(link?.last_error || source.status?.last_error) && (
        <div className="error-text">{source.status?.last_error ?? link?.last_error}</div>
      )}
      <h3 className="subhead">
        Last 60 minutes
        <InfoTip label="Activity">
          Counts over the last hour. {PIPELINE_COUNTS_INFO}
        </InfoTip>
      </h3>
      <div className="counts">
        {totals.length === 0 && <span className="muted">No activity recorded.</span>}
        {totals.map(([k, v]) => (
          <Badge key={k} color={k === 'invalid' || k === 'decode_error' ? 'danger' : 'grey'} size="sm">
            {k} {fmtCount(v)}
          </Badge>
        ))}
      </div>
    </div>
  )
}

function HistoryTab({ id, revision }: { id: string; revision: number }) {
  const [revisions, setRevisions] = useState<Revision[]>([])
  const [selected, setSelected] = useState<number | null>(null)
  useEffect(() => {
    api.revisions(id).then(setRevisions, () => setRevisions([]))
  }, [id, revision])
  const shown = revisions.find((r) => r.revision === selected)
  return (
    <div className="stack" style={{ gap: 8 }}>
      <span className="muted">
        Every save of this source&apos;s settings, newest last. Click one to see it.
        <InfoTip label="Revisions">
          Decision: the number of the decision-log entry that recorded the save, with who made it and the settings before and after.
        </InfoTip>
      </span>
      <DataTable
        aria-label="Revisions"
        columns={REVISION_COLUMNS}
        rows={revisions}
        rowKey={(r) => String(r.revision)}
        selectedKey={selected === null ? null : String(selected)}
        onRowClick={(r) => setSelected(r.revision)}
        maxHeight={200}
      />
      {shown && <CodeEditor aria-label={`Revision ${shown.revision}`} value={JSON.stringify(shown.spec, null, 2)} readOnly maxHeight={360} />}
    </div>
  )
}

export function SourceDetail({
  source,
  open,
  onClose,
  onChanged,
  onDeleted,
}: {
  source: SourceRow
  open: boolean
  onClose: () => void
  onChanged: () => void
  onDeleted: () => void
}) {
  const { toast, confirm } = useToast()
  const [tab, setTab] = useState('status')
  const canAdmin = useCan('admin')
  const [draft, setDraft] = useState<SourceSpec>(source.spec)
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)
  const dirty = useMemo(() => JSON.stringify(draft) !== JSON.stringify(source.spec), [draft, source.spec])
  const state = sourceState(source)

  const save = async () => {
    setSaving(true)
    setSaveError(null)
    try {
      await api.saveSource(draft)
      setSaved(true)
      setTimeout(() => setSaved(false), 1500)
      onChanged()
    } catch (e) {
      setSaveError(errorMessage(e))
    } finally {
      setSaving(false)
    }
  }

  const toggle = async (enabled: boolean) => {
    try {
      await api.setEnabled(source.id, enabled)
      onChanged()
    } catch (e) {
      toast({ variant: 'error', message: errorMessage(e) })
    }
  }

  const remove = async () => {
    const ok = await confirm(`Delete source ${source.id}? Its configuration history stays in the decision log.`, {
      title: 'Delete source',
      confirmLabel: 'Delete',
    })
    if (!ok) return
    try {
      await api.deleteSource(source.id)
      onDeleted()
    } catch (e) {
      toast({ variant: 'error', message: errorMessage(e) })
    }
  }

  return (
    <DetailDrawer
      open={open}
      onClose={onClose}
      label="Source detail"
      storageKey="ot.sourceDetail.width"
      width={760}
      title={source.name}
      status={
        <>
          <Badge color={state.color} size="sm" uppercase>
            {state.label}
          </Badge>
          <InfoTip label="Source state">{SOURCE_STATES_INFO}</InfoTip>
        </>
      }
      actions={
        <>
          <label className="row" style={{ gap: 6 }}>
            <span className="muted">Enabled</span>
            <InfoTip label="Enabled">
              On: the source connects, reads and publishes its tracks. Off: it stops at once; its settings are kept and its tracks age out
              as they go unreported. Saving new settings restarts it.
            </InfoTip>
            <Toggle size="sm" value={source.enabled} disabled={!canAdmin} onChange={toggle} aria-label="Enabled" />
          </label>
          {canAdmin && <SaveButton size="sm" dirty={dirty} saving={saving} saved={saved} onSave={save} />}
          <Button size="xs" variant="ghost" icon={<TbTrash />} disabled={!canAdmin} aria-label="Delete source" title="Delete source" onClick={remove} />
        </>
      }
    >
      <Tabs aria-label="Source views" idPrefix="src" size="sm" value={tab} onChange={setTab} tabs={TABS} style={{ padding: '0 var(--space-md)' }} />
      <div className="panel-body">
        {saveError && <div className="error-text">{saveError}</div>}
        <TabPanel id={tab} idPrefix="src">
          {tab === 'status' && <StatusTab source={source} />}
          {tab === 'transport' && (
            <>
              <TransportForm
                transport={draft.transport}
                codec={draft.pipeline.codec}
                onTransport={(transport) => setDraft({ ...draft, transport })}
                onCodec={(codec) => setDraft({ ...draft, pipeline: { ...draft.pipeline, codec } })}
              />
              <SenderAuth spec={draft} onChange={setDraft} />
            </>
          )}
          {tab === 'pipeline' && (
            <PipelineView
              spec={draft}
              onChange={setDraft}
              sourceId={source.id}
              onSaved={(s) => {
                setDraft(s)
                onChanged()
              }}
            />
          )}
          {tab === 'history' && <HistoryTab id={source.id} revision={source.revision} />}
        </TabPanel>
      </div>
    </DetailDrawer>
  )
}
