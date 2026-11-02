import { useState } from 'react'
import { Badge, RecordTable, SegmentedControl, Tray } from '@kineticlogic/staresdk'
import type { PreviewResult } from '../../../api/client'
import { InfoTip } from '../../../components/InfoTip'
import { PIPELINE_COUNTS_INFO } from '../../../lib/sourceState'
import { countsLine, sampleRows } from '../../../lib/trace'
import { StageSamples } from '../designer/StageSamples'
import type { FlowNode } from './sourceFlow'

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
  error: string | null
  open: boolean
  onOpenChange: (open: boolean) => void
  /** The traced sample being followed through the flow (from 0), if any. */
  followed: number | null
  onFollow: (sample: number | null) => void
}) {
  const [view, setView] = useState<'table' | 'json'>('table')
  const parent = path.length > 1 ? path[path.length - 2] : undefined
  const rows = node && preview ? sampleRows(preview.trace, node.id, parent, path) : []
  const counts = countsLine(preview)
  // A row's sample: `2` or `2-0` (the first of what one sample made).
  const sampleOf = (rowId: string) => Number(rowId.split('-')[0])
  const selected = followed === null ? null : (rows.find((r) => sampleOf(r.id) === followed)?.id ?? null)

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
            A dry run of the source as edited (not yet saved) over its stored samples, as they leave the selected step. In the table,
            fields the step added or changed are tinted and ones it removed are struck through. Click a sample to follow it through the
            flow: the steps it passed light up, and the one that dropped it says why. Nothing is published.
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
        {error && <div className="error-text">{error}</div>}
        {view === 'table' && counts && (
          <div className="value-row">
            <span className="mono muted">{counts}</span>
            <InfoTip label="Sample counts">What the source did with all the stored samples. {PIPELINE_COUNTS_INFO}</InfoTip>
          </div>
        )}
        {view === 'table' && [...new Set(preview?.errors ?? [])].map((e) => (
          <div key={e} className="error-text">
            {e}
          </div>
        ))}
        {!node ? null : !preview ? null : view === 'table' ? (
          <RecordTable
            aria-label={`Samples after ${title}`}
            rows={rows}
            pinned={PINNED}
            // A Map makes observations out of records: everything would differ.
            showChanges={node.type !== 'input' && node.type !== 'map'}
            selectedId={selected}
            onRowClick={(id) => onFollow(sampleOf(id) === followed ? null : sampleOf(id))}
            empty="No stored samples. Capture some on the input."
          />
        ) : (
          <StageSamples result={preview} stageId={node.type === 'output' ? 'publish' : node.id} path={path} />
        )}
      </div>
    </Tray>
  )
}
