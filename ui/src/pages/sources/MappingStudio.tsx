import { useEffect, useMemo, useRef, useState } from 'react'
import { Badge } from '@kineticlogic/staresdk'
import { CodeEditor } from '@kineticlogic/staresdk/code-editor'
import type { Proposal, SourceSpec } from '../../api/client'
import { usePreview } from '../../lib/usePreview'
import { PreviewResults } from './PreviewResults'

interface Props {
  spec: SourceSpec
  onChange: (spec: SourceSpec) => void
  /** Source id whose stored probe samples the preview replays. */
  sampleSourceId: string
  proposals?: Proposal[]
  missing?: string[]
  /** Show the preview's map (the onboarding wizard does; a source's Pipeline view does not). */
  showMap?: boolean
}

/**
 * Mapping studio: edit the pipeline (codec, mapping, enrich stages) as JSON and see the result
 * live against the captured samples.
 */
export function MappingStudio({ spec, onChange, sampleSourceId, proposals, missing, showMap = true }: Props) {
  const [text, setText] = useState(() => JSON.stringify(spec.pipeline, null, 2))
  const [parseError, setParseError] = useState(false)

  // Adopt external spec changes (e.g. a new probe) without clobbering typing.
  const external = useMemo(() => JSON.stringify(spec.pipeline, null, 2), [spec.pipeline])
  const lastEmitted = useRef(external)
  useEffect(() => {
    if (external !== lastEmitted.current) {
      setText(external)
      lastEmitted.current = external
    }
  }, [external])

  const edit = (next: string) => {
    setText(next)
    try {
      const pipeline = JSON.parse(next)
      setParseError(false)
      const updated = { ...spec, pipeline }
      lastEmitted.current = JSON.stringify(pipeline, null, 2)
      onChange(updated)
    } catch {
      setParseError(true)
    }
  }

  const { preview, error: previewError, running } = usePreview(spec, sampleSourceId)

  return (
    <div className="grid-2">
      <div className="stack" style={{ gap: 8 }}>
        {proposals && proposals.length > 0 && (
          <div>
            <h3 className="subhead">Suggested from the probe</h3>
            <ul className="history">
              {proposals.map((p) => (
                <li key={p.target}>
                  <Badge color="grey" size="sm">
                    {Math.round(p.confidence * 100)}%
                  </Badge>
                  <span>
                    <span className="mono">{p.target}</span> <span className="muted">← {p.reason}</span>
                  </span>
                </li>
              ))}
            </ul>
          </div>
        )}
        {missing && missing.length > 0 && (
          <div className="error-text">Not found in the samples, map these by hand: {missing.join(', ')}</div>
        )}
        <CodeEditor aria-label="Pipeline JSON" value={text} onChange={edit} minHeight={260} maxHeight={520} />
        {parseError && <div className="error-text">The JSON does not parse yet; the preview shows the last valid version.</div>}
      </div>
      <div className="stack" style={{ gap: 8 }}>
        <div className="toolbar">
          <h3 className="subhead">Live preview</h3>
          <span className="muted">{running ? 'running…' : `samples of ${sampleSourceId}`}</span>
        </div>
        {previewError && <div className="error-text">{previewError}</div>}
        {preview ? <PreviewResults result={preview} showMap={showMap} /> : !previewError && <span className="muted">Running the first preview…</span>}
      </div>
    </div>
  )
}
