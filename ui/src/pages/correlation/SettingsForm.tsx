import { useEffect, useState } from 'react'
import { Button, FieldSelect, Input, Label } from '@kineticlogic/staresdk'
import { TbPlus, TbTrash } from 'react-icons/tb'
import { JsonField } from '../sources/designer/JsonField'
import { api, type CorrelationSettings, type OutputFilter, type PluginInfo } from '../../api/client'
import { PluginOptions, PluginPicker } from '../../components/PluginOptions'
import { INPUT } from '../../lib/valueSpec'
import { InfoTip } from '../../components/InfoTip'

const APPROACHES = [{ name: 'kinematics_metadata' }, { name: 'kinematics' }, { name: 'identifiers' }]
const MODES = [{ name: 'automatic' }, { name: 'suggest' }]
const YES_NO = [{ name: 'yes' }, { name: 'no' }]

function Row({ label, children, hint }: { label: string; children: React.ReactNode; hint?: string }) {
  return (
    <div className="stage-row">
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

function Num({ label, value, onChange, step, width = 90 }: { label: string; value: number; onChange: (v: number) => void; step?: number; width?: number }) {
  return (
    <Input
      style={{ ...INPUT, width }}
      type="number"
      step={step ?? 'any'}
      aria-label={label}
      value={Number.isFinite(value) ? String(value) : ''}
      onChange={(e) => onChange(e.target.value === '' ? NaN : Number(e.target.value))}
    />
  )
}

/** Correlation settings: how tracks pair, whether pairings are made or proposed, and splits. */
export function SettingsForm({ value, onChange }: { value: CorrelationSettings; onChange: (v: CorrelationSettings) => void }) {
  const k = value.kinematic
  const sp = value.split
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
  useEffect(() => {
    api.plugins().then(setPlugins, () => setPlugins([]))
  }, [])
  const scorers = plugins.filter((p) => p.kinds.includes('scorer'))
  const scorer = value.scorer ?? null
  const scorerPlugin = plugins.find((p) => p.name === scorer?.plugin)
  return (
    <div className="stack">
      <Row label="Pair on" hint="Shared identifiers always pair. Kinematics: agreeing motion too; with metadata, vetoed by conflicting identifiers or domains.">
        <FieldSelect
          ariaLabel="Pairing approach"
          fields={APPROACHES}
          value={value.approach}
          onChange={(v) => onChange({ ...value, approach: (v ?? 'kinematics_metadata') as CorrelationSettings['approach'] })}
          style={{ width: 200 }}
        />
      </Row>
      <Row label="Mode" hint="Suggest: kinematic pairings wait for an operator to accept them.">
        <FieldSelect
          ariaLabel="Pairing mode"
          fields={MODES}
          value={value.mode}
          onChange={(v) => onChange({ ...value, mode: v === 'suggest' ? 'suggest' : 'automatic' })}
          style={{ width: 200 }}
        />
      </Row>
      <h4 className="subhead">
        Kinematic pairing
        <InfoTip label="Kinematic pairing">
          Each comparison propagates the older report&apos;s position, velocity and their uncertainty to the newer one&apos;s time and weighs how well
          they agree against another object being there. The probability of the same object builds from the prior over recent comparisons.
        </InfoTip>
      </h4>
      <Row label="Pair at probability" hint={`From a prior of ${k.prior_probability}, after at least ${k.m} of the last ${k.n} comparisons within ${k.window_secs} s, at least ${k.min_interval_secs} s apart.`}>
        <div className="num-row">
          <Num width={70} label="Pair probability" value={k.pair_probability} onChange={(pair_probability) => setK({ pair_probability })} />
          <span className="muted">prior</span>
          <Num width={70} label="Prior probability" value={k.prior_probability} onChange={(prior_probability) => setK({ prior_probability })} />
        </div>
      </Row>
      <Row
        label="Comparisons"
        hint="How much evidence a pairing needs: at least so many of the last so many comparisons, all within the window (s), each at least the given seconds after the previous one, since consecutive reports are not independent. Defaults: 3 of 5 within 30 s, 3 s apart."
      >
        <div className="num-row">
          <Num width={58} label="Fewest comparisons" value={k.m} step={1} onChange={(m) => setK({ m })} />
          <span className="muted">of the last</span>
          <Num width={58} label="Comparisons" value={k.n} step={1} onChange={(n) => setK({ n })} />
          <span className="muted">within</span>
          <Num width={58} label="Window seconds" value={k.window_secs} onChange={(window_secs) => setK({ window_secs })} />
          <span className="muted">s, apart by</span>
          <Num width={58} label="Seconds between comparisons" value={k.min_interval_secs} onChange={(min_interval_secs) => setK({ min_interval_secs })} />
          <span className="muted">s</span>
        </div>
      </Row>
      <Row label="Gate" hint="Share of true matches a comparison keeps (chi-square on position, and velocity when both report it). Outside it, a comparison counts against the pair.">
        <Num label="Gate probability" value={k.gate_probability} onChange={(gate_probability) => setK({ gate_probability })} />
      </Row>
      <Row label="Another object" hint="How many other objects per km² a report could be, and how spread their velocities are (m/s).">
        <div className="num-row">
          <Num width={70} label="Objects per square kilometre" value={k.object_density_per_km2} onChange={(object_density_per_km2) => setK({ object_density_per_km2 })} />
          <span className="muted">per km², velocities ±</span>
          <Num width={58} label="Velocity spread" value={k.velocity_spread_mps} onChange={(velocity_spread_mps) => setK({ velocity_spread_mps })} />
          <span className="muted">m/s, aircraft ±</span>
          <Num width={58} label="Aircraft velocity spread" value={k.air_velocity_spread_mps} onChange={(air_velocity_spread_mps) => setK({ air_velocity_spread_mps })} />
          <span className="muted">m/s</span>
        </div>
      </Row>
      <Row
        label="Local density radius (m)"
        hint="Counts the live tracks within this distance of a report (the one it is compared with left out) and uses their density instead of Another object's when it is higher: in a crowded harbour a close approach is weaker evidence than at sea. Tracks more than about 5 km away are never counted. 0 turns it off. Default 500 m."
      >
        <Num label="Local density radius" value={k.local_density_radius_m} onChange={(local_density_radius_m) => setK({ local_density_radius_m })} />
      </Row>
      <Row label="Position σ at least (m)" hint="For sources that claim more precision than they have, or report none.">
        <Num label="Minimum sigma" value={k.min_sigma_m} onChange={(min_sigma_m) => setK({ min_sigma_m })} />
      </Row>
      <Row label="Velocity σ (m/s)" hint="For reports with a course and speed but no covariance.">
        <Num label="Velocity sigma" value={k.speed_sigma_mps} onChange={(speed_sigma_mps) => setK({ speed_sigma_mps })} />
      </Row>
      <Row label="Manoeuvre (m/s²)" hint="Process noise: how fast uncertainty grows while a report is propagated in time.">
        <Num label="Process noise" value={k.process_noise_mps2} onChange={(process_noise_mps2) => setK({ process_noise_mps2 })} />
      </Row>
      <Row label="Compare views up to (s)" hint="A report is compared with a system track's view propagated to its time, if the view is at most this old.">
        <Num label="Maximum view age" value={k.max_age_secs} onChange={(max_age_secs) => setK({ max_age_secs })} />
      </Row>
      <Row label="Stopped drift (m/s)" hint="A view that reports no motion (a moored vessel, a parked vehicle) is propagated as a slow drift at this speed instead of a manoeuvring target, so an old position stays useful.">
        <Num label="Stopped drift" value={k.stopped_drift_mps} onChange={(stopped_drift_mps) => setK({ stopped_drift_mps })} />
      </Row>
      <Row
        label="Reuse views"
        hint="Compare a new report with the other side's view already used (a slow feed's last report), its evidence weighted by the new report's share of the uncertainty, at most once every so many seconds: a tracker's consecutive reports are smoothed, not independent."
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
          <Num width={58} label="Reuse interval" value={k.reuse_interval_secs} onChange={(reuse_interval_secs) => setK({ reuse_interval_secs })} />
          <span className="muted">s at most</span>
        </div>
      </Row>
      <Row label="Source live for (s)" hint="A source's track counts as live on a system track this long after its last report; while it does, another track of the same source cannot join (a sensor's two tracks are two objects).">
        <Num label="Source live seconds" value={k.source_live_secs} onChange={(source_live_secs) => setK({ source_live_secs })} />
      </Row>
      <Row
        label="Scorer"
        hint={
          scorers.length
            ? "A scorer plugin's evidence (its likelihood ratio and gate) in place of each kinematic comparison's. The test above, its thresholds and every decision stay OpenTrack's."
            : 'Add a scorer plugin in Settings → General → Plugins to score pairs your own way.'
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
      <h4 className="subhead">
        Identifier pairing
        <InfoTip label="Identifier pairing">
          Tracks that share an identifier (the same mmsi, icao, callsign…) or resolve to the same registry entity with corroboration pair
          whatever the approach and mode, if they pass this sanity gate. Evidence-only schemes (elnot) never pair tracks on their own; a
          shared value only strengthens a kinematic comparison.
        </InfoTip>
      </h4>
      <Row label="Sanity gate (m)" hint="Allowed distance at zero time apart (default 10 000 m), plus the domain's top speed × time apart (surface 40 m/s, ground 70, air or unknown 400, space 8000). A farther identifier match is refused: it stops a reused or spoofed identifier pulling distant objects together.">
        <Num label="Sanity gate" value={value.gate.base_m} onChange={(base_m) => onChange({ ...value, gate: { ...value.gate, base_m } })} />
      </Row>
      <h4 className="subhead">
        Best source
        <InfoTip label="Best source">
          How a paired track picks what to publish from its source tracks. Position: the report with the smallest uncertainty among those
          recent enough, then source priority, then the newest. Name and classification: a registry-corroborated report first, then
          priority.
        </InfoTip>
      </h4>
      <Row label="Freshness (s)" hint="Reports this many seconds or less older than the newest compete on accuracy for the position; older ones are ignored. Default 60 s.">
        <Num label="Freshness" value={value.freshness_secs} onChange={(freshness_secs) => onChange({ ...value, freshness_secs })} />
      </Row>
      <h4 className="subhead">
        Splits
        <InfoTip label="Splits">
          A source track that stops agreeing with the rest of its track (a sensor track that followed the wrong vessel through a crossing, a
          wrongly shared identifier) is split onto a new track, and the two are recorded as different objects so they do not pair again.
        </InfoTip>
      </h4>
      <Row label="Propose splits" hint="Yes: a split waits in Suggestions for an operator to accept or reject (unless Split without asking is on). No and not automatic: no splits at all. Default yes.">
        <FieldSelect ariaLabel="Propose splits" fields={YES_NO} value={sp.propose ? 'yes' : 'no'} onChange={(v) => setS({ propose: v === 'yes' })} style={{ width: 90 }} />
      </Row>
      <Row label="Split without asking" hint="Yes: the engine splits at once and records the decision, whatever Propose splits says. No: splits are only proposed. Default yes.">
        <FieldSelect ariaLabel="Split automatically" fields={YES_NO} value={sp.automatic ? 'yes' : 'no'} onChange={(v) => setS({ automatic: v === 'yes' })} style={{ width: 90 }} />
      </Row>
      <Row
        label="Split at probability"
        hint={`A paired source track's probability of being the same object starts at the pairing threshold and moves with every comparison; split when it falls to this and ${sp.m} of the last ${sp.n} comparisons fall outside a ${sp.gate_probability} gate.`}
      >
        <div className="num-row">
          <Num width={70} label="Split probability" value={sp.split_probability} onChange={(split_probability) => setS({ split_probability })} />
          <span className="muted">and</span>
          <Num width={58} label="Disagreeing comparisons" value={sp.m} step={1} onChange={(m) => setS({ m })} />
          <span className="muted">of</span>
          <Num width={58} label="Split comparisons" value={sp.n} step={1} onChange={(n) => setS({ n })} />
          <span className="muted">outside</span>
          <Num width={70} label="Split gate probability" value={sp.gate_probability} onChange={(gate_probability) => setS({ gate_probability })} />
        </div>
      </Row>
      <Row
        label="Split window (s)"
        hint={`How far back the split's comparisons may reach. A slow feed (AIS every 10 s, a moored vessel every few minutes) gives one comparison per report, so this must hold the last ${sp.n} of them. Never shorter than the pairing window (${k.window_secs} s). Default 300 s.`}
      >
        <Num label="Split window seconds" value={sp.window_secs} onChange={(window_secs) => setS({ window_secs })} />
      </Row>
      <h4 className="subhead">
        Output filter
        <InfoTip label="Output filter">
          What OpenTrack publishes. A track that fails stays inside OpenTrack, marked filtered; a published track that stops passing is deleted
          downstream until it passes again. Empty: everything.
        </InfoTip>
      </h4>
      <Row label="Areas" hint="With any included area, a track must be inside one; it must be outside every excluded area. Degrees; a box may cross the antimeridian (min longitude above max).">
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
              <Num width={70} label="South latitude" value={a.min_lat} onChange={(min_lat) => setArea(i, { min_lat })} />
              <Num width={70} label="West longitude" value={a.min_lon} onChange={(min_lon) => setArea(i, { min_lon })} />
              <span className="muted">to</span>
              <Num width={70} label="North latitude" value={a.max_lat} onChange={(max_lat) => setArea(i, { max_lat })} />
              <Num width={70} label="East longitude" value={a.max_lon} onChange={(max_lon) => setArea(i, { max_lon })} />
              <Button size="xs" variant="ghost" icon={<TbTrash />} aria-label="Remove area" onClick={() => setO({ areas: out.areas.filter((_, j) => j !== i) })} />
            </div>
          ))}
          <div>
            <Button
              size="xs"
              variant="ghost"
              icon={<TbPlus />}
              onClick={() => setO({ areas: [...out.areas, { name: '', exclude: false, min_lat: 0, min_lon: 0, max_lat: 0, max_lon: 0 }] })}
            >
              Add area
            </Button>
          </div>
        </div>
      </Row>
      <Row label="Affiliations" hint="Comma separated, empty for any: pending, unknown, assumed_friend, friend, neutral, suspect, hostile, joker, faker, none.">
        <Input style={{ ...INPUT, width: 320 }} aria-label="Affiliations" defaultValue={out.affiliations.join(', ')} onBlur={(e) => setO({ affiliations: list(e.target.value) })} />
      </Row>
      <Row label="Domains" hint="air, surface, subsurface, ground, space, unknown.">
        <Input style={{ ...INPUT, width: 320 }} aria-label="Domains" defaultValue={out.domains.join(', ')} onBlur={(e) => setO({ domains: list(e.target.value) })} />
      </Row>
      <Row label="Track types" hint="tactical, live_training, simulated_training, demand_entry.">
        <Input style={{ ...INPUT, width: 320 }} aria-label="Track types" defaultValue={out.track_types.join(', ')} onBlur={(e) => setO({ track_types: list(e.target.value) })} />
      </Row>
      <Row label="Confidence at least" hint="The track's confidence, 0 to 1 (0: off).">
        <Num label="Minimum confidence" value={out.min_confidence} onChange={(min_confidence) => setO({ min_confidence })} />
      </Row>
      <Row label="Rule" hint="Anything else, as a condition over the track's fields plus confidence and state.">
        <JsonField label="Output rule" optional value={out.rule} onChange={(v) => setO({ rule: v })} />
      </Row>
    </div>
  )
}
