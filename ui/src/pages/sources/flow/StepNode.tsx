import { useState, type DragEvent, type ReactNode } from 'react'
import { Handle, Position, type NodeProps } from '@xyflow/react'
import { Button, RECORD_FIELD_MIME } from '@kineticlogic/staresdk'
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
  /** The one thing to do next on this step (capture samples, use a suggested mapping). */
  action?: { label: string; icon?: ReactNode; busy?: boolean; onClick: () => void }
  /** A Map takes a field dragged from the samples table: map it. */
  onFieldDrop?: (path: string) => void
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
  const [over, setOver] = useState(false)
  const takesField = (e: DragEvent) => !!step.onFieldDrop && e.dataTransfer.types.includes(RECORD_FIELD_MIME)
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
      data-drop={over || undefined}
      style={{ border: `1px solid ${over ? 'var(--color-accent)' : border}` }}
      onDragOver={(e) => {
        if (!takesField(e)) return
        e.preventDefault()
        e.stopPropagation()
        e.dataTransfer.dropEffect = 'copy'
        setOver(true)
      }}
      onDragLeave={() => setOver(false)}
      onDrop={(e) => {
        if (!takesField(e)) return
        e.preventDefault()
        e.stopPropagation()
        setOver(false)
        step.onFieldDrop?.(e.dataTransfer.getData(RECORD_FIELD_MIME))
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
      {mark?.note && (
        <span className="flow-step-trail" data-state={mark.state}>
          {mark.state === 'dropped' ? 'Dropped here: ' : 'Held here: '}
          {mark.note}
        </span>
      )}
      {step.action && (
        <div className="flow-step-action nodrag nopan">
          <Button
            size="xs"
            variant="secondary"
            icon={step.action.icon}
            disabled={step.action.busy}
            onClick={(e) => {
              e.stopPropagation()
              step.action?.onClick()
            }}
          >
            {step.action.label}
          </Button>
        </div>
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
