import { Badge } from '@kineticlogic/staresdk'
import type { PreviewResult } from '../../../api/client'
import { samplesAt } from '../../../lib/trace'

/**
 * The live preview's samples after one stage, as pretty-printed JSON with any trace notes.
 */
export function StageSamples({ result, stageId, path, ruleFilter, empty }: {
  result: PreviewResult
  stageId: string
  path: string[]
  ruleFilter?: string
  empty: string
}) {
  const samples = samplesAt(result.trace, stageId, path).filter((sample) => !ruleFilter || sample.itemRules?.includes(ruleFilter))
  return (
    <div className="stage-samples">
      {samples.length === 0 && <span className="muted">{empty}</span>}
      {samples.map((s) => (
        <section key={s.title} className="stage-sample">
          <div className="stage-sample-head">
            {s.title}
            {s.matchedRules && s.matchedRules.length > 1 && (
              <Badge size="sm" color="warning">Matched {s.matchedRules.length} rules</Badge>
            )}
          </div>
          {s.notes.map((n, i) => (
            <div key={i} className="muted">
              {n}
            </div>
          ))}
          {s.blocks.map((b, i) => (!ruleFilter || s.itemRules?.[i] === ruleFilter) && (
            <div key={i}>
              {s.itemRules?.[i] && <div className="muted">Rule {s.itemRules[i]}</div>}
              {s.itemDetails?.[i]?.decision && <div className="muted">{s.itemDetails[i].decision}</div>}
              <pre className="stage-sample-json">{b}</pre>
            </div>
          ))}
        </section>
      ))}
    </div>
  )
}
