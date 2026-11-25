import { Fragment } from 'react'
import type { PreviewResult } from '../../../api/client'
import { InfoTip } from '../../../components/InfoTip'
import { sampleRows } from '../../../lib/trace'
import type { FlowNode } from './sourceFlow'

export function StepSampleStatus({ nodeId, connected, pending, preview }: {
  nodeId: string
  connected: boolean
  pending: boolean
  preview: PreviewResult | null
}) {
  if (!connected) return <div className="muted" role="status">Connect an input to preview.</div>
  if (pending) return <div className="muted" role="status">Checking samples…</div>
  const step = preview?.steps?.[nodeId]
  if (!step) return <div className="muted" role="status">No step preview yet.</div>
  if (step.in === 0) return <div className="muted" role="status">No samples reached this step.</div>
  return (
    <div className="value-row">
      <strong>Step samples</strong>
      <span>{step.in.toLocaleString()} in · {step.out.toLocaleString()} out</span>
      <InfoTip label="Step samples">What this step received and passed on during the dry run.</InfoTip>
    </div>
  )
}

const display = (value: unknown) => value == null || value === '' ? '—'
  : typeof value === 'object' ? JSON.stringify(value) : String(value)

export function StepSampleExample({ node, path, preview }: { node: FlowNode; path: string[]; preview: PreviewResult }) {
  if (!['registry', 'affiliation', 'nats'].includes(node.type)) return null
  const rows = sampleRows(preview.trace, node.id, path.at(-2), path).filter((row) => row.value !== undefined)
  const row = node.type === 'registry'
    ? rows.find((r) => !!r.detail?.track_fields?.length || !!r.detail?.entity_updates?.length) ?? rows[0]
    : rows[0]
  if (!row) return null
  const detail = row.detail
  if (node.type === 'nats') {
    return (
      <fieldset className="map-outline">
        <legend>Payload preview</legend>
        <dl className="facts">
          <dt>Subject</dt><dd className="mono">{detail?.subject || '—'}</dd>
          <dt>Sample</dt><dd>{row.label}</dd>
        </dl>
        <pre className="stage-sample-json">{JSON.stringify(row.value, null, 2)}</pre>
      </fieldset>
    )
  }
  return (
    <fieldset className="map-outline">
      <legend>Before and after</legend>
      <dl className="facts">
        <dt>Sample</dt><dd>{row.label}</dd>
        {detail?.decision && <><dt>Result</dt><dd>{detail.decision}</dd></>}
        {node.type === 'affiliation' ? (
          <>
            <dt>Country</dt><dd>{display(detail?.country)}</dd>
            <dt>Before</dt><dd>{display(detail?.before)}</dd>
            <dt>After</dt><dd>{display(detail?.after)}</dd>
          </>
        ) : (
          <>
            {detail?.entity_id && <><dt>Entity</dt><dd className="mono">{detail.entity_id}</dd></>}
            {(detail?.track_fields ?? []).map(({ field, before, after }, index) => (
              <Fragment key={`${field}:${index}`}>
                <dt className="mono">{field}</dt>
                <dd>{display(before)} → {display(after)} · Entity value</dd>
              </Fragment>
            ))}
            {(detail?.entity_updates ?? []).map(([field, value], index) => (
              <Fragment key={`${field}:${index}`}>
                <dt className="mono">{field}</dt><dd>{display(value)} · Feed value for entity</dd>
              </Fragment>
            ))}
            {!detail?.track_fields?.length && !detail?.entity_updates?.length && (
              <>
                <dt>Before (name)</dt><dd>{display(detail?.before)}</dd>
                <dt>After (name)</dt><dd>{display(detail?.after)}</dd>
              </>
            )}
          </>
        )}
      </dl>
    </fieldset>
  )
}
