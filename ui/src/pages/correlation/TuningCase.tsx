import { useCallback } from 'react'
import { Button } from '@kineticlogic/staresdk'
import { TbX } from 'react-icons/tb'
import type { CorrelationSettings, DecisionRow, Suggestion } from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { fmtTime } from '../../lib/format'

export type TuningTarget = 'pair' | 'comparisons' | 'split' | 'identifier'

export interface CorrelationCase {
  label: string
  kind: string
  time: number
  evidence: Record<string, unknown> | null
  reason?: string | null
  target: TuningTarget | null
}

export function suggestionCase(suggestion: Suggestion): CorrelationCase {
  return {
    label: `Suggestion #${suggestion.id}`,
    kind: suggestion.kind,
    time: suggestion.updated_at_ms,
    evidence: suggestion.evidence,
    reason: suggestion.evidence.reason,
    target: suggestion.evidence.rule === 'area-emitter' ? null : suggestion.kind,
  }
}

export function decisionCase(decision: DecisionRow): CorrelationCase | null {
  if (!['pair', 'merge', 'split', 'do_not_pair', 'reject_split'].includes(decision.op)) return null
  const rule = decision.evidence?.rule
  const target = rule === 'identifier' ? 'identifier'
    : rule === 'divergence' ? 'split'
      : rule === 'kinematic' ? 'pair' : null
  return {
    label: `Decision #${decision.id}`,
    kind: decision.op,
    time: decision.at_ms,
    evidence: decision.evidence,
    reason: decision.reason,
    target,
  }
}

type Metric = [label: string, recorded: string, draft?: string]

function numeric(value: unknown, unit = '') {
  return typeof value === 'number' && Number.isFinite(value) ? `${value.toLocaleString()}${unit}` : '—'
}

function percent(value: unknown) {
  return typeof value === 'number' && Number.isFinite(value)
    ? `${(value * 100).toLocaleString(undefined, { maximumSignificantDigits: 4 })}%`
    : '—'
}

function metricsFor(example: CorrelationCase, draft: CorrelationSettings): Metric[] {
  const evidence = example.evidence
  if (example.target === 'identifier') {
    const gate = evidence?.gate as Record<string, unknown> | undefined
    return [
      ['Distance (m)', numeric(gate?.distance_m)],
      ['Allowed distance (m)', numeric(gate?.gate_m)],
      ['Sanity gate (m)', '—', numeric(draft.gate.base_m)],
    ]
  }
  if (example.target === 'split') {
    return [
      ['Same-object probability', percent(evidence?.probability)],
      ['Split at probability', percent(evidence?.split_probability), percent(draft.split.split_probability)],
      ['Comparisons outside gate', numeric(evidence?.outside_gate)],
      ['Disagreeing comparisons required', '—', numeric(draft.split.m)],
      ['Split gate probability', percent(evidence?.split_gate_probability), percent(draft.split.gate_probability)],
      ['Comparisons', numeric(evidence?.comparisons)],
    ]
  }
  if (example.target === 'pair') {
    return [
      ['Same-object probability', percent(evidence?.probability)],
      ['Pair at probability', percent(evidence?.pair_probability), percent(draft.kinematic.pair_probability)],
      ['Comparisons', numeric(evidence?.comparisons)],
      ['Fewest comparisons', '—', numeric(draft.kinematic.m)],
    ]
  }
  if (evidence?.rule === 'area-emitter') {
    return [
      ['Likelihood share', percent(evidence.probability)],
      ['Comparisons', numeric(evidence.comparisons)],
    ]
  }
  return []
}

export function TuningCase({ example, draft, onClear }: {
  example: CorrelationCase
  draft: CorrelationSettings
  onClear: () => void
}) {
  const focusCase = useCallback((section: HTMLElement | null) => { section?.focus() }, [])
  const rows = metricsFor(example, draft).filter(([, recorded, current]) => recorded !== '—' || (current && current !== '—'))
  return (
    <section className="correlation-case" aria-label={example.label} tabIndex={-1} ref={focusCase}>
      <div className="toolbar">
        <strong>{example.label}</strong>
        <span className="muted">{example.kind} · {fmtTime(example.time)}</span>
        <InfoTip label={example.label}>
          {example.reason && <div>{example.reason}</div>}
          <div>Recorded shows what OpenTrack saw or used for this case. Draft shows your current edits. Editing a value does not run the case again.</div>
          {example.target === 'identifier' && (
            <div>Allowed distance includes extra room for the time between reports. Sanity gate shows the distance you entered for reports made at the same time.</div>
          )}
          {!rows.length && <div>OpenTrack did not save any numbers to compare for this case.</div>}
        </InfoTip>
        <Button size="xs" variant="ghost" icon={<TbX />} aria-label="Clear tuning case" onClick={onClear} />
      </div>
      {rows.length > 0 && (
        <table className="correlation-case-values" aria-label={`Tuning values for ${example.label}`}>
          <thead><tr><th scope="col">Metric</th><th scope="col">Recorded</th><th scope="col">Draft</th></tr></thead>
          <tbody>
            {rows.map(([name, recorded, current]) => (
              <tr key={name}><th scope="row">{name}</th><td>{recorded}</td><td>{current ?? '—'}</td></tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  )
}
