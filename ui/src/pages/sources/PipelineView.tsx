import { lazy, Suspense, useEffect, useMemo, useState } from 'react'
import { TbPencil } from 'react-icons/tb'
import { Button, DataTable, TabPanel, Tabs, type DataTableColumn } from '@kineticlogic/staresdk'
import { api, type SchemaOverview, type SourceSpec } from '../../api/client'
import { errorMessage } from '../../lib/format'
import { fieldMap, pipelineStages, type FieldRow } from '../../lib/pipeline'
import { MappingStudio } from './MappingStudio'
import { PipelineDesigner } from './designer/PipelineDesigner'
import { useCan } from '../../auth/context'

// React Flow loads only when the flowchart is shown.
const PipelineFlow = lazy(() => import('./PipelineFlow'))

const VIEWS = [
  { id: 'flow', label: 'Flow' },
  { id: 'json', label: 'JSON' },
]

const COLUMNS: DataTableColumn<FieldRow>[] = [
  { key: 'from', header: 'Source', mono: true, render: (r) => r.from || '—', sortValue: (r) => r.from },
  { key: 'target', header: 'Destination', mono: true, render: (r) => r.target, sortValue: (r) => r.target },
]

/** Each value the mapping sets: the feed field it reads and the OpenTrack field it writes. */
function MapDetail({ spec, schema }: { spec: SourceSpec; schema: SchemaOverview }) {
  // Several rules often set the same field from the same source; list each pair once.
  const rows = useMemo(() => {
    const seen = new Set<string>()
    return fieldMap(spec, schema).filter((r) => {
      const k = `${r.from}>${r.target}`
      return !seen.has(k) && !!seen.add(k)
    })
  }, [spec, schema])
  return <DataTable aria-label="Field map" columns={COLUMNS} rows={rows} rowKey={(r) => r.id} maxHeight={560} empty="This mapping sets no fields." />
}

/**
 * A source's pipeline: a top-to-bottom flowchart of its stages with the selected stage's detail
 * (the Map stage traces every field from the feed to the published message), or the raw JSON
 * with a live preview.
 */
export function PipelineView({
  spec,
  onChange,
  onSaved,
  sourceId,
}: {
  spec: SourceSpec
  onChange: (s: SourceSpec) => void
  /** The designer saved a new revision with this spec. */
  onSaved: (s: SourceSpec) => void
  sourceId: string
}) {
  const [designing, setDesigning] = useState(false)
  const canAdmin = useCan('admin')
  const [view, setView] = useState('flow')
  const [stageId, setStageId] = useState('map')
  const [schema, setSchema] = useState<SchemaOverview | null>(null)
  const [schemaError, setSchemaError] = useState<string | null>(null)

  useEffect(() => {
    api.schema().then(setSchema, (e) => setSchemaError(errorMessage(e)))
  }, [])

  const stages = useMemo(() => pipelineStages(spec), [spec])
  const stage = stages.find((s) => s.id === stageId) ?? stages[0]

  return (
    <div className="stack">
      <div className="value-row">
        <Tabs aria-label="Pipeline views" idPrefix="pipe" size="sm" value={view} onChange={setView} tabs={VIEWS} />
        <span className="spacer" />
        <Button size="sm" variant="secondary" icon={<TbPencil />} disabled={!canAdmin} onClick={() => setDesigning(true)}>
          Edit
        </Button>
      </div>
      {designing && (
        <PipelineDesigner
          initial={spec}
          sourceId={sourceId}
          onClose={() => setDesigning(false)}
          onSaved={(s) => {
            onSaved(s)
            setDesigning(false)
          }}
        />
      )}
      <TabPanel id={view} idPrefix="pipe">
        {view === 'json' ? (
          <MappingStudio spec={spec} onChange={onChange} sampleSourceId={sourceId} showMap={false} />
        ) : (
          <div className="pipeline">
            <Suspense fallback={<span className="muted">LOADING…</span>}>
              <PipelineFlow stages={stages} selected={stage.id} onSelect={setStageId} />
            </Suspense>
            <div className="stack" style={{ minWidth: 0 }}>
              <h3 className="subhead">{stage.title}</h3>
              {stage.id === 'map' ? (
                schema ? (
                  <MapDetail spec={spec} schema={schema} />
                ) : (
                  <span className={schemaError ? 'error-text' : 'muted'}>{schemaError ?? 'LOADING…'}</span>
                )
              ) : (
                <dl className="facts">
                  {stage.facts.map((f) => (
                    <div key={f.label} style={{ display: 'contents' }}>
                      <dt>{f.label}</dt>
                      <dd className="mono">{f.value}</dd>
                    </div>
                  ))}
                </dl>
              )}
            </div>
          </div>
        )}
      </TabPanel>
    </div>
  )
}
