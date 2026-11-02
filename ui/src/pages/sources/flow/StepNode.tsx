import type { ReactNode } from 'react'
import { Handle, Position, type NodeProps } from '@xyflow/react'
import { gives, KIND_WORD, takes, type StepType } from './sourceFlow'

/** What the editor gives each step to draw (the canvas node's `data`). */
export interface StepView extends Record<string, unknown> {
  type: StepType
  icon: ReactNode
  title: string
  summary: string
  problems: string[]
  /** What the dry run passed through it: taken in and passed on. */
  counts?: { in: number; out: number }
  /** While a sample is followed: whether it passed this step, was dropped or held here, or never came. */
  trail?: { state: 'passed' | 'dropped' | 'held'; note?: string } | 'absent'
}

/**
 * One step of a source on the workspace: its icon, heading and one line of its settings, a
 * connection in unless it is the input and out unless it is an output, and the first thing that
 * stops it running in red (its settings list them all).
 */
export function StepNode({ data, selected, isConnectable }: NodeProps) {
  const step = data as StepView
  const failing = step.problems.length > 0
  // A step that takes samples and passes none on is worth a look (a Filter that drops everything).
  const stops = !!step.counts && step.counts.in > 0 && step.counts.out === 0
  const mark = typeof step.trail === 'object' ? step.trail : undefined
  const border = mark?.state === 'dropped'
    ? 'var(--color-destructive)'
    : mark
      ? 'var(--color-accent)'
      : failing
        ? 'var(--color-destructive)'
        : selected
          ? 'var(--color-accent)'
          : stops
            ? 'var(--status-warning)'
            : 'var(--border-strong)'
  return (
    <div
      className="flow-step"
      data-failing={failing || undefined}
      data-trail={mark?.state ?? (step.trail === 'absent' ? 'absent' : undefined)}
      style={{ border: `1px solid ${border}` }}
    >
      {takes(step.type) && <Handle type="target" position={Position.Left} isConnectable={isConnectable} />}
      <div className="flow-step-head">
        <span aria-hidden className="flow-step-icon">
          {step.icon}
        </span>
        <span className="flow-step-title">{step.title}</span>
        <span className="flow-step-kind">{KIND_WORD[step.type]}</span>
      </div>
      {step.summary && <span className="flow-step-summary">{step.summary}</span>}
      {mark?.note && (
        <span className="flow-step-trail" data-state={mark.state}>
          {mark.state === 'dropped' ? 'Dropped here: ' : 'Held here: '}
          {mark.note}
        </span>
      )}
      {step.counts && (
        <span className="flow-step-counts" data-stops={stops || undefined} title={`${step.counts.in} in, ${step.counts.out} out over the stored samples`}>
          {step.counts.in} → {step.counts.out}
        </span>
      )}
      {failing && (
        <span role="alert" className="flow-step-problem" title={step.problems.join('\n')}>
          {step.problems[0]}
        </span>
      )}
      {step.problems.length > 1 && <span className="flow-step-problem-more">+{step.problems.length - 1} more</span>}
      {gives(step.type) && <Handle type="source" position={Position.Right} isConnectable={isConnectable} />}
    </div>
  )
}
