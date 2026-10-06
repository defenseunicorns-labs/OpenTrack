import type { PreviewResult } from '../../../api/client'
import { InfoTip } from '../../../components/InfoTip'
import { PIPELINE_COUNTS_INFO } from '../../../lib/sourceState'
import { countsLine, samplesAt } from '../../../lib/trace'

/**
 * The live preview's samples after one stage: a line of counts and any errors, then each traced
 * sample as pretty-printed JSON (or what became of it). The list scrolls, not the modal.
 */
export function StageSamples({ result, stageId }: { result: PreviewResult; stageId: string }) {
  const samples = samplesAt(result.trace, stageId)
  const errors = [...new Set(result.errors ?? [])]
  const counts = countsLine(result)
  return (
    <>
      <div className="value-row">
        <span className="mono muted">{counts || 'No counts yet.'}</span>
        <InfoTip label="Preview counts">What the pipeline did with all the stored samples. {PIPELINE_COUNTS_INFO}</InfoTip>
      </div>
      {errors.map((e) => (
        <div key={e} className="error-text">
          {e}
        </div>
      ))}
      <div className="stage-samples">
        {samples.length === 0 && <span className="muted">No stored samples. Capture some from the feed.</span>}
        {samples.map((s) => (
          <section key={s.title} className="stage-sample">
            <div className="stage-sample-head">{s.title}</div>
            {s.notes.map((n, i) => (
              <div key={i} className="muted">
                {n}
              </div>
            ))}
            {s.blocks.map((b, i) => (
              <pre key={i} className="stage-sample-json">
                {b}
              </pre>
            ))}
          </section>
        ))}
      </div>
    </>
  )
}
