import { useState } from 'react'
import { Badge, RecordTable, SegmentedControl, Select, Tray } from '@kineticlogic/staresdk'
import type { DecodedSamples, PreviewResult } from '../../../api/client'
import { InfoTip } from '../../../components/InfoTip'
import { PIPELINE_COUNTS_INFO } from '../../../lib/sourceState'
import { countsLine, sampleRows } from '../../../lib/trace'
import { StageSamples } from '../designer/StageSamples'
import { INPUT, type FlowNode } from './sourceFlow'
import { StepSampleStatus } from './StepPreview'

/** Fields shown first in a sample table, when the samples have them. */
const PINNED = ['source_track_key', 'name', 'observed_at', 'position.latitude', 'position.longitude']

/**
 * The selected step's samples, in a tray along the bottom of the workspace: a dry run of the source
 * as edited over its stored samples, as they leave the step. As a table, each sample is a row with
 * what the step changed from what reached it highlighted; as JSON, each sample whole.
 */
export function SamplesTray({
  node,
  title,
  path,
  preview,
  previewPending,
  decodedSamples,
  decodedError,
  error,
  open,
  onOpenChange,
  followed,
  onFollow,
}: {
  node: FlowNode | null
  title: string
  /** The steps from the input to the selected one. */
  path: string[]
  preview: PreviewResult | null
  previewPending: boolean
  decodedSamples: DecodedSamples | null
  decodedError: string | null
  error: string | null
  open: boolean
  onOpenChange: (open: boolean) => void
  /** The traced sample being followed through the flow (from 0), if any. */
  followed: number | null
  onFollow: (sample: number | null) => void
}) {
  const [view, setView] = useState<'table' | 'json'>('table')
  const [selection, setSelection] = useState<{ step: string; rule: string } | null>(null)
  const connected = path[0] === INPUT
  const mapping = node?.type === 'map'
  const throttle = node?.type === 'throttle'
  const output = node?.type === 'output'
  const stageId = output ? 'publish' : node?.id
  const stagePath = output ? [...path, 'publish'] : path
  const rules = mapping ? (node.config as { mapping?: { rules?: { name?: string }[] } } | undefined)?.mapping?.rules : undefined
  const names = [...new Set((rules ?? []).map((rule) => rule.name).filter((name): name is string => !!name))]
  const ruleFilter = selection && selection.step === node?.id && names.includes(selection.rule) ? selection.rule : undefined
  const ready = connected && !previewPending
  const inputFallback = ready && !!node && !preview?.trace && ['input', 'reject', 'map'].includes(node.type) && !!decodedSamples
  const rows = ready && stageId ? sampleRows(preview?.trace, stageId, stagePath.at(-2), stagePath, throttle) : []
  const counts = ready ? countsLine(preview) : ''
  // A row's sample: `2` or `2-0` (the first of what one sample made).
  const sampleOf = (rowId: string) => Number(rowId.split('-')[0])
  const overlaps = mapping ? new Set(rows.filter((r) => (r.matchedRules?.length ?? 0) > 1).map((r) => sampleOf(r.id))).size : 0
  const visibleRows = ruleFilter ? rows.filter((r) => r.rule === ruleFilter) : rows
  const selected = followed === null ? null : (visibleRows.find((r) => sampleOf(r.id) === followed)?.id ?? null)
  const tableRows = mapping ? visibleRows.map((r) => {
    const matches = r.matchedRules
    const overlap = matches && matches.length > 1
    return {
      ...r,
      value: r.value && typeof r.value === 'object'
        ? { Rule: r.rule, Overlap: overlap ? matches.join(', ') : undefined, ...r.value }
        : r.value,
      dropped: r.dropped && overlap ? `${r.dropped} · Matches: ${matches.join(', ')}` : r.dropped,
    }
  }) : throttle ? visibleRows.map((r) => ({
    ...r,
    value: r.value && typeof r.value === 'object' ? { Decision: r.detail?.decision, ...r.value } : r.value,
  })) : visibleRows
  const empty = ruleFilter ? 'No traced samples produced output for this rule.' : 'No samples to show for this step.'

  return (
    <Tray
      aria-label="Samples"
      title={node ? `Samples · ${title}` : 'Samples'}
      open={open}
      onOpenChange={onOpenChange}
      height={240}
      minHeight={120}
      maxHeight={560}
      storageKey="ot.sources.samples.height"
      actions={
        <>
          {followed !== null && (
            <Badge size="sm" color="brand" onRemove={() => onFollow(null)} removeLabel="Stop following">
              Following #{followed + 1}
            </Badge>
          )}
          <InfoTip label="Samples">
            A dry run of the source as edited (not yet saved) over its stored samples, as they leave the selected step. While a Map is incomplete,
            the Input, Reject and Map steps show decoded input records for reference. In the table,
            fields the step added or changed are tinted and ones it removed are struck through. Click a sample to follow it through the
            flow: the steps it passed light up, and the one that dropped it says why. On the input, drag a column onto a Map to map that
            field. Output shows the message that would be published using the active schema. Its track ID is a placeholder for the one assigned by correlation.
            The dry run publishes nothing.
          </InfoTip>
          <SegmentedControl
            aria-label="Samples view"
            size="sm"
            value={view}
            onChange={setView}
            options={[
              { value: 'table', label: 'Table' },
              { value: 'json', label: 'JSON' },
            ]}
          />
        </>
      }
    >
      <div className="samples-tray-body">
        {node && <StepSampleStatus nodeId={node.id} connected={connected} pending={previewPending} preview={preview} />}
        {ready && error && <div className="error-text">{error}</div>}
        {counts && (
          <div className="value-row">
            <span className="mono muted">{counts}</span>
            <InfoTip label="Sample counts">What the source did with all the stored samples. {PIPELINE_COUNTS_INFO}</InfoTip>
          </div>
        )}
        {ready && mapping && preview?.trace && names.length > 0 && (
          <div className="value-row">
            <Select
              ariaLabel="Map rule"
              options={[{ value: '', label: 'All rules' }, ...names.map((name) => ({ value: name, label: name }))]}
              value={ruleFilter ?? ''}
              onChange={(rule) => setSelection(rule ? { step: node.id, rule } : null)}
              style={{ width: 180 }}
            />
          </div>
        )}
        {overlaps > 0 && (
          <div className="value-row">
            <Badge size="sm" color="warning">{overlaps} sampled {overlaps === 1 ? 'record' : 'records'} matched multiple rules</Badge>
            <InfoTip label="Rule overlap">
              This counts overlaps across all rules. Overlap lists the rules that matched, and Rule identifies which one produced each row. A rule can match without producing output.
            </InfoTip>
          </div>
        )}
        {ready && [...new Set(preview?.errors ?? [])].map((e) => (
          <div key={e} className="error-text">
            {e}
          </div>
        ))}
        {inputFallback && decodedSamples?.last_decode_error && <div className="error-text">{decodedSamples.last_decode_error}</div>}
        {inputFallback && decodedError && <div className="error-text">{decodedError}</div>}
        {inputFallback && decodedSamples && (
          <div className="value-row">
            <span className="mono muted">Input samples before mapping · {decodedSamples.frames} frame(s) · {decodedSamples.records.length} decoded record(s)</span>
          </div>
        )}
        {!node || !ready ? null : inputFallback ? (
          view === 'table' ? (
            <RecordTable
              aria-label="Decoded input samples"
              rows={decodedSamples.records.map((value, i) => ({ id: String(i), label: `#${i + 1}`, value }))}
              draggableColumns
              empty="No decoded records. Capture samples on the input."
            />
          ) : <pre className="mono">{JSON.stringify(decodedSamples.records, null, 2)}</pre>
        ) : !preview || !stageId ? null : view === 'table' ? (
          <RecordTable
            aria-label={`Samples after ${title}`}
            rows={tableRows}
            pinned={mapping ? ['Rule', 'Overlap', ...PINNED] : throttle ? ['Decision', ...PINNED] : output ? ['track_id', ...PINNED] : PINNED}
            // A Map makes observations out of records: everything would differ.
            showChanges={node.type !== 'input' && !mapping && !output}
            // A record's fields can be dragged onto a Map to map them.
            draggableColumns={node.type === 'input' || node.type === 'reject'}
            selectedId={selected}
            onRowClick={(id) => onFollow(sampleOf(id) === followed ? null : sampleOf(id))}
            empty={empty}
          />
        ) : (
          <StageSamples result={preview} stageId={stageId} path={stagePath} ruleFilter={ruleFilter} empty={empty} />
        )}
      </div>
    </Tray>
  )
}
