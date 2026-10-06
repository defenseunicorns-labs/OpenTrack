import { Label } from '@kineticlogic/staresdk'
import { InfoTip } from '../../components/InfoTip'

/** A labelled settings row, its explanation in an ⓘ. */
export function SettingsRow({ label, hint, children }: { label: string; hint?: React.ReactNode; children: React.ReactNode }) {
  return (
    <div className="settings-row">
      <div className="row-label">
        <Label size="sm">{label}</Label>
        {hint && <InfoTip label={label}>{hint}</InfoTip>}
      </div>
      <div className="settings-row-control">{children}</div>
    </div>
  )
}
