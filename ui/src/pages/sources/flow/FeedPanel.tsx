import { TbX } from 'react-icons/tb'
import { Button, Input, Label } from '@kineticlogic/staresdk'
import type { SourceRow, SourceSpec } from '../../../api/client'
import { InfoTip } from '../../../components/InfoTip'
import { EmitterMotionForm, PublishForm, SecurityForm } from '../designer/StageForms'
import { SourceHistory, SourceStatus } from '../SourceStatus'

/**
 * The feed's own settings, for everything it reports: its name and description, what it reports
 * and how correlation takes it, its priority and security label; then its status and history.
 */
export function FeedPanel({
  spec,
  row,
  tracked,
  problems,
  readOnly,
  onChange,
  onClose,
}: {
  spec: SourceSpec
  row: SourceRow
  /** Whether a Tracker comes before the Output. */
  tracked: boolean
  problems: string[]
  readOnly: boolean
  onChange: (s: SourceSpec) => void
  onClose: () => void
}) {
  const without = <K extends keyof SourceSpec>(key: K): SourceSpec => {
    const next = { ...spec }
    delete next[key]
    return next
  }
  return (
    <div className="node-panel">
      <div className="node-panel-head">
        <span className="flow-step-kind">Feed</span>
        <h3 className="subhead">{spec.name || spec.id}</h3>
        <span className="spacer" />
        <Button size="xs" variant="ghost" icon={<TbX />} aria-label="Close" title="Close" onClick={onClose} />
      </div>
      <div className="node-panel-body">
        {problems.map((m) => (
          <div key={m} className="error-text" role="alert">
            {m}
          </div>
        ))}
        <fieldset className="step-settings stack" disabled={readOnly}>
          <div className="form-grid">
            <div className="field">
              <Label size="sm" htmlFor="feed-name">
                Name
              </Label>
              <Input id="feed-name" value={spec.name} onChange={(e) => onChange({ ...spec, name: e.target.value })} />
            </div>
            <div className="field">
              <Label size="sm" htmlFor="feed-id">
                Id
              </Label>
              <Input id="feed-id" value={spec.id} disabled className="mono" />
            </div>
            <div className="field wide">
              <Label size="sm" htmlFor="feed-description">
                Description
              </Label>
              <Input
                id="feed-description"
                value={spec.description ?? ''}
                onChange={(e) => onChange(e.target.value ? { ...spec, description: e.target.value } : without('description'))}
              />
            </div>
          </div>
          <PublishForm
            reports={spec.reports}
            tracker={tracked}
            publishAlone={spec.publish_alone}
            confirmAfter={spec.confirm_after}
            onChange={(reports) => onChange(reports ? { ...spec, reports } : without('reports'))}
            onPublishAlone={(publish_alone) => onChange({ ...spec, publish_alone })}
            onConfirmAfter={(confirm_after) => onChange({ ...spec, confirm_after })}
          />
          <div className="stage-row">
            <div className="row-label">
              <Label size="sm" htmlFor="feed-priority">
                Priority
              </Label>
              <InfoTip label="Priority">
                Where several sources report one track, priority breaks ties: it picks the position among equally certain reports, the
                identity and classification when no report is corroborated by the registry, and each extension field. Higher wins. Default 100.
              </InfoTip>
            </div>
            <Input
              id="feed-priority"
              type="number"
              style={{ width: 90 }}
              value={spec.priority === undefined ? '' : String(spec.priority)}
              placeholder="100"
              onChange={(e) =>
                onChange(e.target.value.trim() === '' ? without('priority') : { ...spec, priority: Math.round(Number(e.target.value)) })
              }
            />
          </div>
          <SecurityForm value={spec.security} onChange={(security) => onChange({ ...spec, security })} />
          <EmitterMotionForm value={spec.emitter_motion} onChange={(emitter_motion) => onChange({ ...spec, emitter_motion })} />
        </fieldset>
        <h3 className="subhead">Status</h3>
        <SourceStatus source={row} />
        <h3 className="subhead">History</h3>
        <SourceHistory id={row.id} revision={row.revision} />
      </div>
    </div>
  )
}
