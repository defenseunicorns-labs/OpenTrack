import { useCallback, useEffect, useRef, useState } from 'react'
import { TbDeviceFloppy, TbDownload, TbFileImport, TbTrash } from 'react-icons/tb'
import { Button, FieldSelect, Input, Label, Modal, Toggle, useToast } from '@kineticlogic/staresdk'
import { api, type TrackerProfile } from '../../../api/client'
import { InfoTip } from '../../../components/InfoTip'
import { errorMessage } from '../../../lib/format'
import { INPUT } from '../../../lib/valueSpec'

type Obj = Record<string, unknown>

const KINDS = ['gmti', 'maritime_mti', 'surface_radar', 'air_radar', 'lidar', 'sonar', 'eo_ir', 'other']

function download(p: TrackerProfile) {
  const { builtin: _builtin, ...file } = p
  const blob = new Blob([JSON.stringify(file, null, 2) + '\n'], { type: 'application/json' })
  const a = document.createElement('a')
  a.href = URL.createObjectURL(blob)
  a.download = `${p.name}.json`
  a.click()
  URL.revokeObjectURL(a.href)
}

/**
 * Whether a tracker differs from a profile in what the profile sets (the
 * server fills in defaults the profile leaves out; the key prefix is the
 * source's own).
 */
function differs(profile: unknown, value: unknown, top = true): boolean {
  if (profile && typeof profile === 'object' && !Array.isArray(profile)) {
    const v = (value && typeof value === 'object' ? value : {}) as Obj
    return Object.entries(profile as Obj).some(([k, x]) => !(top && k === 'key_prefix') && differs(x, v[k], false))
  }
  if (typeof profile === 'number' && typeof value === 'number') return Math.abs(profile - value) > 1e-12 * Math.max(1, Math.abs(profile))
  return JSON.stringify(profile) !== JSON.stringify(value)
}

/** The tracker without what only records where it came from. */
function settingsOf(tracker: Obj): Obj {
  const { profile: _profile, ...rest } = tracker
  return rest
}

/** Save the tracker stage as a profile (or import a profile file). */
function SaveProfile({ initial, onClose, onSaved }: { initial: TrackerProfile; onClose: () => void; onSaved: (p: TrackerProfile) => void }) {
  const { toast } = useToast()
  const [p, setP] = useState<TrackerProfile>(initial)
  const [replace, setReplace] = useState(false)
  const [busy, setBusy] = useState(false)
  const set = (patch: Partial<TrackerProfile>) => setP({ ...p, ...patch })
  const save = async () => {
    setBusy(true)
    try {
      const saved = await api.saveTrackerProfile({ ...p, builtin: undefined }, replace)
      toast({ variant: 'success', title: 'Profile saved', message: saved.label })
      onSaved(saved)
    } catch (e) {
      toast({ variant: 'error', title: 'Profile not saved', message: errorMessage(e) })
    } finally {
      setBusy(false)
    }
  }
  const row = (label: string, hint: string | undefined, control: React.ReactNode) => (
    <div className="settings-row">
      <div className="row-label">
        <Label size="sm">{label}</Label>
        {hint && <InfoTip label={label}>{hint}</InfoTip>}
      </div>
      <div className="settings-row-control">{control}</div>
    </div>
  )
  return (
    <Modal title="Save tracker profile" onClose={onClose} width={620}>
      <div className="panel-body stack">
        {row('Name', 'The file name: lowercase letters, digits, - and _.',
          <Input style={{ ...INPUT, width: 240 }} aria-label="Profile name" value={p.name} onChange={(e) => set({ name: e.target.value.toLowerCase().replace(/[^a-z0-9_-]/g, '-') })} spellCheck={false} />)}
        {row('Label', 'A readable name, shown when the profile is loaded and in the Profile ⓘ. Required.', <Input style={{ ...INPUT, width: 360 }} aria-label="Profile label" value={p.label} onChange={(e) => set({ label: e.target.value })} />)}
        {row('Sensor', 'What kind of sensor it is for, so it is easy to find.',
          <span className="num-row">
            <FieldSelect ariaLabel="Sensor kind" fields={KINDS.map((name) => ({ name }))} value={String(p.sensor.kind ?? 'other')} onChange={(k) => set({ sensor: { ...p.sensor, kind: k ?? 'other' } })} style={{ width: 150 }} />
            <Input style={{ ...INPUT, width: 200 }} aria-label="Platform" placeholder="platform (optional)" value={String(p.sensor.platform ?? '')} onChange={(e) => set({ sensor: { ...p.sensor, platform: e.target.value || undefined } })} />
          </span>)}
        {row('Description', 'What the profile is for (sensor, targets, conditions); shown in the Profile ⓘ when it is picked.', <textarea className="plain-textarea" aria-label="Description" rows={2} value={p.description} onChange={(e) => set({ description: e.target.value })} />)}
        {row('Basis', 'Where the numbers come from: a benchmark run, a sensor manual, experience.',
          <textarea className="plain-textarea" aria-label="Basis" rows={2} value={p.basis} onChange={(e) => set({ basis: e.target.value })} />)}
        {row('Replace', 'Overwrite an imported profile of the same name (the shipped ones are never overwritten).', <Toggle size="sm" aria-label="Replace" value={replace} onChange={setReplace} />)}
        <div className="num-row" style={{ justifyContent: 'flex-end' }}>
          <Button size="sm" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" icon={<TbDeviceFloppy />} disabled={busy || !p.name || !p.label.trim()} onClick={save}>
            Save
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/**
 * Tracker profiles for a tracker stage: load one (the stage takes its
 * settings), save the stage as one, import a file, download one.
 */
export function TrackerProfiles({ value, onChange, row }: { value: Obj; onChange: (v: Obj) => void; row: (label: string, hint: string, control: React.ReactNode) => React.ReactNode }) {
  const { toast, confirm } = useToast()
  const [profiles, setProfiles] = useState<TrackerProfile[]>([])
  const [picked, setPicked] = useState<string | null>((value.profile as string | undefined) ?? null)
  const [saving, setSaving] = useState<TrackerProfile | null>(null)
  const file = useRef<HTMLInputElement>(null)
  const load = useCallback(() => {
    api.trackerProfiles().then((r) => setProfiles(r.profiles), () => setProfiles([]))
  }, [])
  useEffect(load, [load])
  const current = profiles.find((p) => p.name === picked)
  const from = value.profile as string | undefined
  const fromProfile = profiles.find((p) => p.name === from)
  const changed = fromProfile != null && differs(fromProfile.tracker, value)

  const apply = async () => {
    if (!current) return
    const ok = await confirm(`The tracker stage takes ${current.label}'s settings; its track key prefix stays.`, {
      title: 'Load profile',
      confirmLabel: 'Load',
    })
    if (!ok) return
    onChange({ ...current.tracker, key_prefix: value.key_prefix ?? current.tracker.key_prefix, profile: current.name })
  }
  const importFile = async (f: File | undefined) => {
    if (!f) return
    try {
      const p = JSON.parse(await f.text()) as TrackerProfile
      setSaving({ ...p, builtin: undefined })
    } catch (e) {
      toast({ variant: 'error', title: 'Not a profile', message: errorMessage(e) })
    }
  }
  const remove = async () => {
    if (!current || current.builtin) return
    if (!(await confirm(`Delete the imported profile ${current.label}? Trackers loaded from it keep their settings.`, { title: 'Delete profile', confirmLabel: 'Delete' }))) return
    try {
      await api.deleteTrackerProfile(current.name)
      setPicked(null)
      load()
    } catch (e) {
      toast({ variant: 'error', title: 'Not deleted', message: errorMessage(e) })
    }
  }

  const hint =
    (current ? `${current.description} ${current.basis ? `Basis: ${current.basis}` : ''}` : 'A sensor’s tracker settings, as a file: load one to start from settings made for the sensor, and save a tuned tracker as one to share it.') +
    (from ? ` This stage started from ${fromProfile?.label ?? from}${changed ? ', and has changed since' : ''}.` : '')
  return (
    <>
      {row(
        'Profile',
        hint,
        <span className="num-row">
          <FieldSelect
            ariaLabel="Tracker profile"
            allowNone
            fields={profiles.map((p) => ({ name: p.name }))}
            value={picked}
            onChange={(n) => setPicked(n)}
            style={{ width: 220 }}
          />
          <Button size="xs" variant="ghost" disabled={!current} onClick={apply}>
            Load
          </Button>
          <Button size="xs" variant="ghost" icon={<TbDownload />} title="Download the profile" aria-label="Download profile" disabled={!current} onClick={() => current && download(current)} />
          <Button size="xs" variant="ghost" icon={<TbTrash />} title="Delete (imported profiles only)" aria-label="Delete profile" disabled={!current || current.builtin} onClick={remove} />
          <Button
            size="xs"
            variant="ghost"
            icon={<TbDeviceFloppy />}
            title="Save this tracker as a profile"
            aria-label="Save as profile"
            onClick={() =>
              setSaving({
                name: fromProfile && !fromProfile.builtin ? fromProfile.name : '',
                label: fromProfile && !fromProfile.builtin ? fromProfile.label : '',
                description: '',
                sensor: { ...(fromProfile?.sensor ?? {}), kind: fromProfile?.sensor.kind ?? 'other' },
                basis: fromProfile ? `Tuned from ${fromProfile.label}.` : '',
                tracker: settingsOf(value),
              })
            }
          />
          <Button size="xs" variant="ghost" icon={<TbFileImport />} title="Import a profile file" aria-label="Import profile" onClick={() => file.current?.click()} />
          <input ref={file} type="file" accept=".json,application/json" hidden onChange={(e) => importFile(e.target.files?.[0])} />
          {from && <span className="muted small">{changed ? `from ${from} (changed)` : `from ${from}`}</span>}
        </span>,
      )}
      {saving && (
        <SaveProfile
          initial={saving}
          onClose={() => setSaving(null)}
          onSaved={(p) => {
            setSaving(null)
            setPicked(p.name)
            load()
          }}
        />
      )}
    </>
  )
}
