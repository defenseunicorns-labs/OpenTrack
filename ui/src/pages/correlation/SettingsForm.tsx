import { useEffect, useId, useRef, useState } from 'react'
import { Button, FieldSelect, Input, Label } from '@kineticlogic/staresdk'
import { TbPlus, TbTrash } from 'react-icons/tb'
import { JsonField } from '../sources/designer/JsonField'
import { api, type CorrelationSettings, type OutputFilter, type PluginInfo } from '../../api/client'
import { PluginOptions, PluginPicker } from '../../components/PluginOptions'
import { INPUT } from '../../lib/valueSpec'
import { InfoTip } from '../../components/InfoTip'
import type { TuningTarget } from './TuningCase'

const APPROACHES = [{ name: 'kinematics_metadata' }, { name: 'kinematics' }, { name: 'identifiers' }]
const MODES = [{ name: 'automatic' }, { name: 'suggest' }]
const YES_NO = [{ name: 'yes' }, { name: 'no' }]

function SettingsSection({ title, hint, children }: {
  title: string
  hint: React.ReactNode
  children: React.ReactNode
}) {
  return (
    <fieldset className="correlation-settings-section" aria-label={title}>
      <legend>{title}<InfoTip label={title}>{hint}</InfoTip></legend>
      {children}
    </fieldset>
  )
}

function Row({ label, children, hint, target, highlighted }: {
  label: string
  children: React.ReactNode
  hint?: string
  target?: TuningTarget
  highlighted?: boolean
}) {
  return (
    <div className={highlighted ? 'stage-row correlation-tuning-highlight' : 'stage-row'} data-tuning-target={target}>
      <div className="row-label">
        <Label size="sm">{label}</Label>
        {hint && <InfoTip label={label}>{hint}</InfoTip>}
      </div>
      <div className="stack" style={{ gap: 2 }}>
        {children}
      </div>
    </div>
  )
}

function Num({ label, value, onChange, step, width = 90, unit, showLabel }: {
  label: string
  value: number
  onChange: (v: number) => void
  step?: number
  width?: number
  unit?: string
  showLabel?: boolean
}) {
  const unitId = useId()
  const input = (
    <Input
      style={{ ...INPUT, width }}
      type="number"
      step={step ?? 'any'}
      aria-label={label}
      aria-describedby={unit ? unitId : undefined}
      value={Number.isFinite(value) ? String(value) : ''}
      onChange={(e) => onChange(e.target.value === '' ? NaN : Number(e.target.value))}
    />
  )
  const control = unit ? (
    <span className="correlation-input-unit">
      {input}
      <span id={unitId} className="muted">({unit})</span>
    </span>
  ) : input
  return showLabel ? (
    <label className="correlation-number-field">
      <span>{label}</span>
      {control}
    </label>
  ) : control
}

/** Correlation settings: how tracks pair, whether pairings are made or proposed, and splits. */
export function SettingsForm({ value, onChange, caseTarget, shortcut }: {
  value: CorrelationSettings
  onChange: (v: CorrelationSettings) => void
  caseTarget?: TuningTarget | null
  shortcut: { target: TuningTarget } | null
}) {
  const k = value.kinematic
  const sp = value.split
  const kinematic = value.approach !== 'identifiers'
  const splits = sp.propose || sp.automatic
  const setK = (patch: Partial<CorrelationSettings['kinematic']>) => onChange({ ...value, kinematic: { ...k, ...patch } })
  const setS = (patch: Partial<CorrelationSettings['split']>) => onChange({ ...value, split: { ...sp, ...patch } })
  const out: OutputFilter = value.output ?? { areas: [], affiliations: [], domains: [], track_types: [], min_confidence: 0 }
  const setO = (patch: Partial<OutputFilter>) => onChange({ ...value, output: { ...out, ...patch } })
  const list = (v: string) =>
    v
      .split(',')
      .map((x) => x.trim())
      .filter(Boolean)
  const setArea = (i: number, patch: Partial<OutputFilter['areas'][number]>) =>
    setO({ areas: out.areas.map((a, j) => (j === i ? { ...a, ...patch } : a)) })
  const [plugins, setPlugins] = useState<PluginInfo[]>([])
  const form = useRef<HTMLDivElement>(null)
  const highlighted = shortcut?.target ?? caseTarget
  useEffect(() => {
    if (!shortcut) return
    const row = form.current?.querySelector(`[data-tuning-target="${shortcut.target}"]`)
    row?.scrollIntoView({ block: 'nearest' })
    row?.querySelector<HTMLElement>('input')?.focus({ preventScroll: true })
  }, [shortcut])
  useEffect(() => {
    api.plugins().then(setPlugins, () => setPlugins([]))
  }, [])
  const scorers = plugins.filter((p) => p.kinds.includes('scorer'))
  const scorer = value.scorer ?? null
  const scorerPlugin = plugins.find((p) => p.name === scorer?.plugin)
  return (
    <div className="panels" ref={form}>
      <SettingsSection title="Review" hint="Choose what can make tracks pair and when you want to review a pairing or split. Pairings based on movement and splits have separate controls.">
        <Row label="Pair on" hint="Identifiers uses shared IDs, like a callsign. Kinematics also allows matches based on movement. Kinematics with metadata checks movement too, but rules out matches when details conflict, such as one track being in the air and the other on the ground.">
          <FieldSelect
            ariaLabel="Pairing approach"
            fields={APPROACHES}
            value={value.approach}
            onChange={(v) => onChange({ ...value, approach: (v ?? 'kinematics_metadata') as CorrelationSettings['approach'] })}
            style={{ width: 200 }}
          />
        </Row>
        {kinematic && (
          <Row label="Mode" hint="In suggest mode, matches based on movement go to Suggestions for you to review. In automatic mode, OpenTrack applies them right away. Matches based on a shared identifier can still happen automatically in either mode.">
            <FieldSelect
              ariaLabel="Pairing mode"
              fields={MODES}
              value={value.mode}
              onChange={(v) => onChange({ ...value, mode: v === 'suggest' ? 'suggest' : 'automatic' })}
              style={{ width: 200 }}
            />
          </Row>
        )}
        <Row label="Propose splits" hint="When Split without asking is off, choose yes to review possible splits in Suggestions. Choose no to turn splits off.">
          <FieldSelect ariaLabel="Propose splits" fields={YES_NO} value={sp.propose ? 'yes' : 'no'} onChange={(v) => setS({ propose: v === 'yes' })} style={{ width: 90 }} />
        </Row>
        <Row label="Split without asking" hint="Choose yes to let OpenTrack split tracks right away and record them in Decisions. Choose no to let Propose splits control whether they go to review.">
          <FieldSelect ariaLabel="Split automatically" fields={YES_NO} value={sp.automatic ? 'yes' : 'no'} onChange={(v) => setS({ automatic: v === 'yes' })} style={{ width: 90 }} />
        </Row>
      </SettingsSection>
      {kinematic && (
        <SettingsSection title="Kinematic pairing" hint="OpenTrack compares where objects are and how they move. It estimates where the older report would put the object at the time of the newer report. Several comparisons help it decide whether both tracks show the same object.">
          <Row label="Pair at probability" target="pair" highlighted={highlighted === 'pair'} hint="How sure does OpenTrack need to be that two tracks show the same object? Raise the first number to require stronger evidence. Lower it to allow pairings with weaker evidence. The prior is OpenTrack's starting estimate before any comparisons.">
            <div className="num-row">
              <Num width={70} label="Pair probability" value={k.pair_probability} onChange={(pair_probability) => setK({ pair_probability })} />
              <span className="muted">prior</span>
              <Num width={70} label="Prior probability" value={k.prior_probability} onChange={(prior_probability) => setK({ prior_probability })} />
            </div>
          </Row>
          <Row
            label="Comparisons"
            target="comparisons"
            highlighted={highlighted === 'comparisons'}
            hint="Set how many comparisons OpenTrack needs and how many recent ones it can use. A longer window gives slow feeds more time to reach the minimum. The last number sets how far apart the comparisons must be so updates that come in quick succession do not count as separate evidence."
          >
            <div className="num-row">
              <Num width={58} label="Fewest comparisons" value={k.m} step={1} onChange={(m) => setK({ m })} />
              <span className="muted">of the last</span>
              <Num width={58} label="Comparisons" value={k.n} step={1} onChange={(n) => setK({ n })} />
              <span className="muted">within</span>
              <Num width={58} label="Window seconds" unit="seconds" value={k.window_secs} onChange={(window_secs) => setK({ window_secs })} />
              <span className="muted">apart by</span>
              <Num width={58} label="Seconds between comparisons" unit="seconds" value={k.min_interval_secs} onChange={(min_interval_secs) => setK({ min_interval_secs })} />
            </div>
          </Row>
          <Row label="Gate" hint="How closely do two reports need to agree to pass a single comparison? A higher number allows more difference in position or movement. A report outside that limit counts against the match.">
            <Num label="Gate probability" value={k.gate_probability} onChange={(gate_probability) => setK({ gate_probability })} />
          </Row>
          <Row label="Another object" hint="Tell OpenTrack how crowded the area is and how much objects there vary in speed and direction. When more objects are nearby, two reports being close is weaker evidence that they show the same object. Aircraft have their own movement setting.">
            <div className="correlation-number-fields">
              <Num showLabel width={70} label="Objects per square kilometre" unit="objects per square kilometer" value={k.object_density_per_km2} onChange={(object_density_per_km2) => setK({ object_density_per_km2 })} />
              <Num showLabel width={58} label="Velocity spread" unit="meters/second" value={k.velocity_spread_mps} onChange={(velocity_spread_mps) => setK({ velocity_spread_mps })} />
              <Num showLabel width={58} label="Aircraft velocity spread" unit="meters/second" value={k.air_velocity_spread_mps} onChange={(air_velocity_spread_mps) => setK({ air_velocity_spread_mps })} />
            </div>
          </Row>
          <Row
            label="Local density radius"
            hint="Set how far around a report OpenTrack looks for other live tracks. It uses that count to estimate how crowded the area is, and uses the local estimate when it is higher than the one in Another object. The search stops at about 5 km. Enter 0 to turn this off."
          >
            <Num label="Local density radius" unit="meters" value={k.local_density_radius_m} onChange={(local_density_radius_m) => setK({ local_density_radius_m })} />
          </Row>
          <Row label="Position σ at least" hint="Set the smallest position error OpenTrack should allow for. If a source leaves out its error estimate or gives a smaller one, OpenTrack uses this value. A higher number allows for less precise positions.">
            <Num label="Minimum sigma" unit="meters" value={k.min_sigma_m} onChange={(min_sigma_m) => setK({ min_sigma_m })} />
          </Row>
          <Row label="Velocity σ" hint="Set how much error to allow when a source reports speed and direction without saying how accurate they are. Raise it if those movement reports are less reliable.">
            <Num label="Velocity sigma" unit="meters/second" value={k.speed_sigma_mps} onChange={(speed_sigma_mps) => setK({ speed_sigma_mps })} />
          </Row>
          <Row label="Manoeuvre" hint="OpenTrack estimates where an object moved between reports. A higher value allows more room for changes in speed or direction since the last report.">
            <Num label="Process noise" unit="meters/second^2" value={k.process_noise_mps2} onChange={(process_noise_mps2) => setK({ process_noise_mps2 })} />
          </Row>
          <Row label="Compare views up to" hint="Set how old a track's last update can be and still count in a comparison. A higher number lets OpenTrack compare against older updates.">
            <Num label="Maximum view age" unit="seconds" value={k.max_age_secs} onChange={(max_age_secs) => setK({ max_age_secs })} />
          </Row>
          <Row label="Stopped drift" hint="Even a stopped object might shift a little between reports. This sets how quickly OpenTrack lets the possible position spread out when the last report says the object is stopped.">
            <Num label="Stopped drift" unit="meters/second" value={k.stopped_drift_mps} onChange={(stopped_drift_mps) => setK({ stopped_drift_mps })} />
          </Row>
          <Row
            label="Reuse views"
            hint="Choose yes to let a new report be compared with an older update that OpenTrack has already used. This helps when one source reports less often. The seconds setting limits how often repeat comparisons count."
          >
            <div className="num-row">
              <FieldSelect
                ariaLabel="Reuse views"
                fields={YES_NO}
                value={k.reuse_views === false ? 'no' : 'yes'}
                onChange={(v) => setK({ reuse_views: v !== 'no' })}
                style={{ width: 80 }}
              />
              <span className="muted">every</span>
              <Num width={58} label="Reuse interval" unit="seconds" value={k.reuse_interval_secs} onChange={(reuse_interval_secs) => setK({ reuse_interval_secs })} />
              <span className="muted">at most</span>
            </div>
          </Row>
          <Row label="Source live for" hint="Set how long a source's track still counts as active after its last report. During that time, the combined track cannot take a second track from the same source.">
            <Num label="Source live seconds" unit="seconds" value={k.source_live_secs} onChange={(source_live_secs) => setK({ source_live_secs })} />
          </Row>
          <Row
            label="Scorer"
            hint={
              scorers.length
                ? 'Choose how each movement comparison is scored. A plugin can supply its own score. OpenTrack still uses your settings to decide when to pair tracks.'
                : 'OpenTrack scores each movement comparison. To use a different scoring tool, add a scorer plugin under Settings → General → Plugins.'
            }
          >
            <div className="num-row">
              <FieldSelect
                ariaLabel="Pair scorer"
                fields={[{ name: 'kinematic' }, { name: 'plugin', disabled: !scorers.length && !scorer, disabledReason: 'no scorer plugins' }]}
                value={scorer ? 'plugin' : 'kinematic'}
                onChange={(v) => onChange({ ...value, scorer: v === 'plugin' ? { plugin: scorers[0]?.name ?? '', options: structuredClone(scorers[0]?.default_options ?? {}) } : null })}
                style={{ width: 130 }}
              />
              {scorer && (
                <PluginPicker
                  plugins={plugins}
                  kind="scorer"
                  value={scorer.plugin}
                  onChange={(p) => onChange({ ...value, scorer: { plugin: p.name, options: structuredClone(p.default_options) } })}
                  width={200}
                />
              )}
            </div>
          </Row>
          {scorer && (
            <PluginOptions
              plugin={scorerPlugin}
              value={scorer.options ?? {}}
              onChange={(options) => onChange({ ...value, scorer: { ...scorer, options } })}
              row={(key, label, help, control) => (
                <Row key={key} label={label} hint={help}>
                  {control}
                </Row>
              )}
            />
          )}
        </SettingsSection>
      )}
      <SettingsSection title="Identifier pairing" hint="OpenTrack can pair tracks automatically when they share an ID, like a callsign, and their positions make sense. A verified match to the same registry entry can also count. Some IDs, such as ELNOT, can support a movement match but cannot create a match by themselves.">
        <Row label="Sanity gate" target="identifier" highlighted={highlighted === 'identifier'} hint="Even when two reports share an identifier, their positions still need to make sense. This sets how far apart they can be when reported at the same time. OpenTrack allows more distance when more time has passed between reports.">
          <Num label="Sanity gate" unit="meters" value={value.gate.base_m} onChange={(base_m) => onChange({ ...value, gate: { ...value.gate, base_m } })} />
        </Row>
      </SettingsSection>
      <SettingsSection title="Best source" hint="When several sources report the same object, OpenTrack uses the most accurate recent position. If reports tie, it checks source priority, then picks the newest. For the name and classification, it first looks for details backed up by the registry.">
        <Row label="Freshness" hint="Set how much older a position report can be than the newest one and still be considered. A shorter time keeps the position more current. A longer time lets older, more accurate reports stay in the running.">
          <Num label="Freshness" unit="seconds" value={value.freshness_secs} onChange={(freshness_secs) => onChange({ ...value, freshness_secs })} />
        </Row>
      </SettingsSection>
      {splits && (
        <SettingsSection title="Splits" hint="A split can happen when one source's reports stop agreeing with the others. For example, a sensor might switch to following the wrong boat when two boats cross paths. After a split, OpenTrack remembers that the two tracks show different objects.">
          <Row
            label="Split at probability"
            target="split"
            highlighted={highlighted === 'split'}
            hint="Lower Split probability to require stronger evidence before splitting a track. Split comparisons sets how many recent checks to look at. Disagreeing comparisons sets how many of those checks must disagree. Split gate probability controls how different reports need to be to count as a disagreement."
          >
            <div className="correlation-number-fields">
              <Num showLabel width={70} label="Split probability" value={sp.split_probability} onChange={(split_probability) => setS({ split_probability })} />
              <Num showLabel width={58} label="Disagreeing comparisons" value={sp.m} step={1} onChange={(m) => setS({ m })} />
              <Num showLabel width={58} label="Split comparisons" value={sp.n} step={1} onChange={(n) => setS({ n })} />
              <Num showLabel width={70} label="Split gate probability" value={sp.gate_probability} onChange={(gate_probability) => setS({ gate_probability })} />
            </div>
          </Row>
          <Row
            label="Split window"
            hint="Set how far back OpenTrack can look for comparisons when checking for a split. A longer window gives slow feeds time to report enough times. OpenTrack uses the pairing window if it is longer."
          >
            <Num label="Split window seconds" unit="seconds" value={sp.window_secs} onChange={(window_secs) => setS({ window_secs })} />
          </Row>
        </SettingsSection>
      )}
      <SettingsSection title="Output filter" hint="Choose which tracks OpenTrack sends to other systems. Filtered tracks stay visible inside OpenTrack. If a track stops passing the filter, OpenTrack removes it from its published output until it passes again. With no filters, OpenTrack sends every track.">
        <Row label="Areas" hint="Use include to send tracks inside your chosen boxes. Use exclude to block tracks inside a box, even if another box includes them. Enter coordinates in degrees. For a box that crosses the date line, enter a west longitude greater than its east longitude.">
          <div className="stack" style={{ gap: 4 }}>
            {out.areas.map((a, i) => (
              <div key={i} className="num-row">
                <Input style={{ ...INPUT, width: 110 }} aria-label="Area name" placeholder="name" value={a.name} onChange={(e) => setArea(i, { name: e.target.value })} />
                <FieldSelect
                  ariaLabel="Include or exclude"
                  fields={[{ name: 'include' }, { name: 'exclude' }]}
                  value={a.exclude ? 'exclude' : 'include'}
                  onChange={(v) => setArea(i, { exclude: v === 'exclude' })}
                  style={{ width: 100 }}
                />
                <Num width={70} label="South latitude" unit="degrees" value={a.min_lat} onChange={(min_lat) => setArea(i, { min_lat })} />
                <Num width={70} label="West longitude" unit="degrees" value={a.min_lon} onChange={(min_lon) => setArea(i, { min_lon })} />
                <span className="muted">to</span>
                <Num width={70} label="North latitude" unit="degrees" value={a.max_lat} onChange={(max_lat) => setArea(i, { max_lat })} />
                <Num width={70} label="East longitude" unit="degrees" value={a.max_lon} onChange={(max_lon) => setArea(i, { max_lon })} />
                <Button size="xs" variant="ghost" icon={<TbTrash />} aria-label="Remove area" onClick={() => setO({ areas: out.areas.filter((_, j) => j !== i) })} />
              </div>
            ))}
            <div>
              <Button
                size="xs"
                variant="ghost"
                icon={<TbPlus />}
                  aria-label="Add area"
                onClick={() => setO({ areas: [...out.areas, { name: '', exclude: false, min_lat: 0, min_lon: 0, max_lat: 0, max_lon: 0 }] })}
              >
                Add area
              </Button>
            </div>
          </div>
        </Row>
        <Row label="Affiliations" hint="Choose which affiliations to send. Leave this blank to allow all, or enter one or more values separated by commas. Values: pending, unknown, assumed_friend, friend, neutral, suspect, hostile, joker, faker, none.">
          <Input key={out.affiliations.join(', ')} style={{ ...INPUT, width: 320 }} aria-label="Affiliations" defaultValue={out.affiliations.join(', ')} onBlur={(e) => setO({ affiliations: list(e.target.value) })} />
        </Row>
        <Row label="Domains" hint="Choose where tracks can come from. Leave this blank to allow all, or enter values separated by commas. Values: air, surface, subsurface, ground, space, unknown.">
          <Input key={out.domains.join(', ')} style={{ ...INPUT, width: 320 }} aria-label="Domains" defaultValue={out.domains.join(', ')} onBlur={(e) => setO({ domains: list(e.target.value) })} />
        </Row>
        <Row label="Track types" hint="Choose which types of tracks to send. Leave this blank to allow all, or enter values separated by commas. Values: tactical, live_training, simulated_training, demand_entry.">
          <Input key={out.track_types.join(', ')} style={{ ...INPUT, width: 320 }} aria-label="Track types" defaultValue={out.track_types.join(', ')} onBlur={(e) => setO({ track_types: list(e.target.value) })} />
        </Row>
        <Row label="Confidence at least" hint="Only send tracks that reach this confidence score. Use a number from 0 to 1. For example, 0.8 means 80%. Enter 0 to turn this filter off.">
          <Num label="Minimum confidence" value={out.min_confidence} onChange={(min_confidence) => setO({ min_confidence })} />
        </Row>
        <Row label="Rule" hint="Add a custom check for which tracks to send. You can check a track's details, its confidence, or its state. Leave this blank if the other filters cover what you need.">
          <JsonField label="Output rule" optional value={out.rule} onChange={(v) => setO({ rule: v })} />
        </Row>
      </SettingsSection>
    </div>
  )
}
