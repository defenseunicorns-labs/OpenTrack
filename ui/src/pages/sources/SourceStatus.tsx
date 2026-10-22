import { useEffect, useState } from 'react'
import { Badge, DataTable, type DataTableColumn } from '@kineticlogic/staresdk'
import { CodeEditor } from '@kineticlogic/staresdk/code-editor'
import { api, type MetricsResponse, type SourceRow } from '../../api/client'
import { ago, fmtCount, fmtNum, fmtTime } from '../../lib/format'
import { PIPELINE_COUNTS_INFO } from '../../lib/sourceState'
import { InfoTip } from '../../components/InfoTip'

type Revision = Awaited<ReturnType<typeof api.revisions>>[number]

const REVISION_COLUMNS: DataTableColumn<Revision>[] = [
  { key: 'rev', header: 'Revision', width: 80, render: (r) => r.revision, sortValue: (r) => r.revision },
  { key: 'at', header: 'Saved', mono: true, render: (r) => fmtTime(r.saved_at_ms) },
  { key: 'decision', header: 'Decision', width: 90, align: 'right', render: (r) => (r.decision_id ? `#${r.decision_id}` : '—') },
]

/** A saved source's live status: its link, revision and the last hour's counts. */
export function SourceStatus({ source }: { source: SourceRow }) {
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
        {((source.spec.pipeline.nodes as { type?: string; config?: { auto_timing?: unknown } }[] | undefined) ?? []).some((n) => n.type === 'tracker' && n.config?.auto_timing != null) && (
          <>
            <dt>Tracker timing</dt>
            <dd>
              {timing
                ? `revisit ${fmtNum(timing.revisit_secs, 1)} s (${timing.revisit_source}) · confirm within ${fmtNum(timing.confirm_within_secs, 0)} s · drop after ${fmtNum(timing.drop_tentative_secs, 0)} s unconfirmed, ${fmtNum(timing.drop_confirmed_secs, 0)} s confirmed`
                : 'waiting for the sensor’s revisit rate'}
            </dd>
          </>
        )}
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

/** Every saved revision of a source, each viewable. */
export function SourceHistory({ id, revision }: { id: string; revision: number }) {
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
