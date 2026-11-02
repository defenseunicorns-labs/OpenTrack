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
  return (
    <div
      className="flow-step"
      data-failing={failing || undefined}
      style={{
        border: `1px solid ${failing ? 'var(--color-destructive)' : selected ? 'var(--color-accent)' : stops ? 'var(--status-warning)' : 'var(--border-strong)'}`,
      }}
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
