import { useEffect, useState } from 'react'
import { TbPlus, TbTrash, TbX } from 'react-icons/tb'
import { Button, FieldSelect, Input, Label, Toggle } from '@kineticlogic/staresdk'
import { api, type EmitterMotion, type PluginInfo, type SecurityLabel } from '../../../api/client'
import { DECODE_CODECS, DEFAULT_GRADES, DEFAULT_LINKS, describeCondition, describeValue, entityLinks, transportCodec, type EntityLink, type ValueSpec } from '../../../lib/pipeline'
import { JsonField } from './JsonField'
import { ConditionBuilder } from './ConditionBuilder'
import type { SampleField } from '../../../lib/conditions'
import { INPUT } from '../../../lib/valueSpec'
import { ValueEditor } from './ValueEditor'
import { InfoTip } from '../../../components/InfoTip'
import { PluginOptions, PluginPicker } from '../../../components/PluginOptions'
import { TrackerProfiles } from './TrackerProfiles'

type Obj = Record<string, unknown>
type Props = { value: Obj; onChange: (v: Obj) => void }

const words = (s: string) =>
  s
    .split(/[\s,]+/)
    .map((x) => x.trim())
    .filter(Boolean)
const num = (s: string) => (s.trim() === '' ? undefined : Number(s))

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

/** How frames become records. */
export function DecodeForm({ value, onChange }: Props) {
  const type = String(value.type ?? 'json')
  const fixed = transportCodec(value)
  if (fixed)
    return (
      <div className="stack">
        <Row
          label="Format"
          hint="This codec is chosen on the source's Transport tab (Codec), with its .proto files and message or its plugin and options; change it there. Picking a format here would replace them."
        >
          <span className="mono">{fixed.name ? `${fixed.type} · ${fixed.name}` : fixed.type}</span>
        </Row>
      </div>
    )
  return (
    <div className="stack">
      <Row
        label="Format"
        hint="How each frame becomes records. json: a JSON object or array. cot_xml: one record per Cursor-on-Target <event>. xml: one record per element with the name set below. Protobuf and codec plugins are chosen on the source's Transport tab (Codec)."
      >
        <FieldSelect
          ariaLabel="Codec"
          fields={DECODE_CODECS.map((name) => ({ name }))}
          value={type}
          onChange={(t) => onChange(t === 'xml' ? { type: t, record_element: '' } : { type: t ?? 'json' })}
          style={{ width: 160 }}
        />
      </Row>
      {type === 'json' && (
        <>
          <Row label="Records at" hint="Path to the array of records in each frame (adsb.lol: ac). Empty: the frame is the record, or each element of an array.">
            <Input
              style={INPUT}
              aria-label="Records path"
              value={String(value.records ?? '')}
              onChange={(e) => onChange({ ...value, records: e.target.value || undefined })}
              spellCheck={false}
            />
          </Row>
          <Row label="Frame fields" hint="Frame-level fields copied into every record under _frame (e.g. now), comma-separated.">
            <Input
              style={INPUT}
              aria-label="Frame context fields"
              value={((value.context as string[]) ?? []).join(', ')}
              onChange={(e) => {
                const c = words(e.target.value)
                onChange({ ...value, context: c.length ? c : undefined })
              }}
              spellCheck={false}
            />
          </Row>
        </>
      )}
      {type === 'xml' && (
        <Row label="Record element" hint="One record per element with this name.">
          <Input style={INPUT} aria-label="Record element" value={String(value.record_element ?? '')} onChange={(e) => onChange({ ...value, record_element: e.target.value })} spellCheck={false} />
        </Row>
      )}
    </div>
  )
}

/** Records dropped before mapping, each with a reason (counted in the source's metrics). */
export function RejectForm({ rules, onChange }: { rules: Obj[]; onChange: (r: Obj[]) => void }) {
  return (
    <div className="stack">
      {rules.map((r, i) => (
        <div key={i} className="map-field">
          <div className="value-row">
            <Label size="sm">Reason</Label>
            {i === 0 && (
              <InfoTip label="Reason">
                Checked against each raw record before mapping; the first rule whose condition holds drops the record. The reason names it in the
                source&apos;s metrics as <span className="mono">rejected:&lt;reason&gt;</span> (and in the total <span className="mono">rejected</span>), so
                use a short word such as <span className="mono">no_position</span>.
              </InfoTip>
            )}
            <Input style={{ ...INPUT, width: 200 }} aria-label={`Reject rule ${i + 1} reason`} value={String(r.reason ?? '')} onChange={(e) => onChange(rules.map((x, j) => (j === i ? { ...x, reason: e.target.value } : x)))} />
            <span className="spacer" />
            <Button size="xs" variant="ghost" icon={<TbTrash />} aria-label={`Remove reject rule ${i + 1}`} title="Remove" onClick={() => onChange(rules.filter((_, j) => j !== i))} />
          </div>
          <JsonField
            label={`Reject rule ${i + 1} condition`}
            optional
            value={r.when}
            placeholder='{"path": "lat", "exists": false}'
            onChange={(v) => onChange(rules.map((x, j) => (j === i ? { ...x, when: v } : x)))}
            describe={(v) => `drop when ${describeCondition(v)}`}
          />
        </div>
      ))}
      <div>
        <Button size="sm" variant="secondary" icon={<TbPlus />} onClick={() => onChange([...rules, { reason: 'reason', when: { path: '', exists: false } }])}>
          Reject rule
        </Button>
      </div>
    </div>
  )
}

export function JoinForm({ value, onChange }: Props) {
  const hours = Number(value.ttl_secs ?? 7 * 86400) / 3600
  return (
    <Row label="Keep identity for" hint="Hours a track's static fields stay usable after its last static report.">
      <Input style={{ ...INPUT, width: 120 }} type="number" aria-label="Identity kept for hours" value={String(hours)} onChange={(e) => onChange({ ttl_secs: Math.round(Number(e.target.value || 0) * 3600) })} />
    </Row>
  )
}

const GRADES = ['exact', 'hull', 'name', 'generic', 'stale']
const DIRECTIONS = [{ name: 'entity → track' }, { name: 'track → entity' }]

/** The entity stage: how a track finds its entity, and which fields flow which way at a corroborated match. */
export function RegistryForm({ value, onChange }: Props) {
  const links = entityLinks(value)
  const setLinks = (next: EntityLink[]) => {
    const rest = Object.fromEntries(Object.entries(value).filter(([k]) => k !== 'apply'))
    onChange({ ...rest, links: next })
  }
  const setLink = (i: number, patch: Partial<EntityLink>) => setLinks(links.map((l, j) => (j === i ? { ...l, ...patch } : l)))
  const [entityFields, setEntityFields] = useState<string[]>([])
  const [trackFields, setTrackFields] = useState<string[]>([])
  useEffect(() => {
    api.registryFields().then(
      (f) => setEntityFields([...f.minimum, 'entity_id', ...f.attributes.map((a) => a.key)]),
      () => {},
    )
    api.schema().then(
      (o) => {
        const ext = (o.versions.find((v) => v.version === o.latest_published)?.fields ?? []).filter((f) => !f.builtin).map((f) => `ext.${f.key}`)
        setTrackFields([...o.core, ...ext])
      },
      () => {},
    )
  }, [])
  return (
    <div className="stack">
      <Row label="Schemes" hint="Identifier schemes to resolve, in priority order. Empty: every identifier the track carries.">
        <Input style={INPUT} aria-label="Registry schemes" value={((value.schemes as string[]) ?? []).join(', ')} onChange={(e) => onChange({ ...value, schemes: words(e.target.value) })} spellCheck={false} />
      </Row>
      <Row label="Name to grade" hint="The track field compared with the entity's name and each identifier's broadcast name, to grade the match.">
        <Input style={INPUT} aria-label="Broadcast name field" value={String(value.broadcast_name ?? 'name')} onChange={(e) => onChange({ ...value, broadcast_name: e.target.value })} spellCheck={false} />
      </Row>
      <Row
        label="Use at grades"
        hint={`How well the broadcast name matches the entity found by identifier; the links below are used only at the grades listed here (default: ${DEFAULT_GRADES.join(', ')}). exact: the name equals the identifier's expected broadcast name. hull: it contains a number from the entity's hull code. name: its distinctive words (4 letters or more in all) all appear in the entity's name. generic: only generic words, one of them marking a warship, and the entity is military. stale: none of these, or no name at all. Choose from ${GRADES.join(', ')}.`}
      >
        <Input
          style={INPUT}
          aria-label="Apply grades"
          value={((value.apply_grades as string[]) ?? DEFAULT_GRADES).join(', ')}
          onChange={(e) => onChange({ ...value, apply_grades: words(e.target.value) })}
          spellCheck={false}
        />
      </Row>
      <div className="entity-section-head">
        <h4 className="subhead">
          Links
          <InfoTip label="Links">
            Entity → track: the entity is the authority, and its value replaces what the feed reports (the difference is shown on the track).
            Track → entity: the feed updates the entity, e.g. an AIS destination. A mapping can also send a value straight to the entity with an{' '}
            <span className="mono">entity.&lt;key&gt;</span> destination. Default links replaces the list with the defaults: the entity&apos;s OTH-GOLD
            minimum (name, class name, domain, affiliation, track type, CoT type, SIDC) populates the track.
          </InfoTip>
        </h4>
        <div className="num-row">
          <Button size="sm" variant="ghost" onClick={() => setLinks(DEFAULT_LINKS)}>
            Default links
          </Button>
          <Button size="sm" variant="ghost" icon={<TbPlus />} onClick={() => setLinks([...links, { entity: '', track: '', direction: 'to_track' }])}>
            Add link
          </Button>
        </div>
      </div>
      <datalist id="link-entity-fields">
        {entityFields.map((f) => (
          <option key={f} value={f} />
        ))}
      </datalist>
      <datalist id="link-track-fields">
        {trackFields.map((f) => (
          <option key={f} value={f} />
        ))}
      </datalist>
      <div className="kv-table kv-links">
        <span className="kv-head">Entity field</span>
        <span className="kv-head">Direction</span>
        <span className="kv-head">Track field</span>
        <span />
        {links.map((l, i) => (
          <div key={i} className="kv-row">
            <Input
              style={{ ...INPUT, width: '100%', fontFamily: 'var(--font-mono)' }}
              aria-label={`Link ${i + 1} entity field`}
              list="link-entity-fields"
              value={l.entity}
              placeholder="name, flag…"
              onChange={(e) => setLink(i, { entity: e.target.value })}
              spellCheck={false}
            />
            <FieldSelect
              ariaLabel={`Link ${i + 1} direction`}
              fields={DIRECTIONS}
              value={l.direction === 'to_entity' ? 'track → entity' : 'entity → track'}
              onChange={(v) => setLink(i, { direction: v === 'track → entity' ? 'to_entity' : 'to_track' })}
              style={{ width: '100%' }}
            />
            <Input
              style={{ ...INPUT, width: '100%', fontFamily: 'var(--font-mono)' }}
              aria-label={`Link ${i + 1} track field`}
              list="link-track-fields"
              value={l.track}
              placeholder="classification.domain"
              onChange={(e) => setLink(i, { track: e.target.value })}
              spellCheck={false}
            />
            <Button size="xs" variant="ghost" icon={<TbX />} aria-label={`Remove link ${i + 1}`} onClick={() => setLinks(links.filter((_, j) => j !== i))} />
          </div>
        ))}
        {links.length === 0 && <span className="muted kv-empty">No links: the entity is matched, but nothing flows either way.</span>}
      </div>
    </div>
  )
}

const AFFILIATIONS = ['friend', 'hostile', 'neutral', 'unknown', 'suspect', 'assumed_friend', 'pending']

export function AffiliationForm({ value, onChange }: Props) {
  const listInput = (key: string, label: string, hint: string) => (
    <Row label={label} hint={hint}>
      <Input style={INPUT} aria-label={`${label} countries`} placeholder="ISO codes: US GB FR" value={((value[key] as string[]) ?? []).join(' ')} onChange={(e) => onChange({ ...value, [key]: words(e.target.value.toUpperCase()) })} spellCheck={false} />
    </Row>
  )
  const pick = (key: string, label: string, hint: string) => (
    <Row label={label} hint={hint}>
      <FieldSelect ariaLabel={label} allowNone fields={AFFILIATIONS.map((name) => ({ name }))} value={(value[key] as string) ?? null} onChange={(v) => onChange({ ...value, [key]: v ?? undefined })} style={{ width: 160 }} />
    </Row>
  )
  return (
    <div className="stack">
      <Row label="Country from" hint={`The value holding the track's ISO country code (compared in capitals, spaces trimmed), e.g. the registry flag. Currently: ${describeValue(value.country as ValueSpec)}`}>
        <ValueEditor label="Country" value={value.country as ValueSpec} onChange={(v) => onChange({ ...value, country: v })} />
      </Row>
      {listInput('friendly', 'Friend', 'Country codes, separated by spaces or commas, whose tracks get affiliation friend. It replaces any affiliation the mapping or entity set.')}
      {listInput('hostile', 'Hostile', 'Country codes whose tracks get affiliation hostile.')}
      {listInput('neutral', 'Neutral', 'Country codes whose tracks get affiliation neutral.')}
      {pick('otherwise', 'Other countries', 'Affiliation for a track whose country is known but in none of the lists. Empty: left as it is.')}
      {pick('unknown_country', 'No country', 'Affiliation for a track with no country code. Empty: left as it is.')}
    </div>
  )
}

/**
 * Keep or drop observations, built as rows of field, test and value from the fields the samples
 * hold at this stage. `effect`: what the dry run says the filter as edited does to the samples.
 */
export function FilterForm({
  value,
  onChange,
  fields,
  effect,
}: Props & { fields: SampleField[]; effect: { dropped: number; examples: string[] } | null }) {
  return (
    <div className="stack">
      <Row
        label="Keep only if"
        hint="Observations that do not match are dropped and counted as filtered. Empty: keep all. Fields are the observation after mapping (and ext.registry.* after the entity stage); the list offers those in the stored samples, and Other path… takes any."
      >
        <ConditionBuilder label="Keep condition" verb="keep" value={value.keep_if} onChange={(v) => onChange({ ...value, keep_if: v })} fields={fields} />
      </Row>
      <Row
        label="Drop if"
        hint="Observations that match are dropped (counted as filtered), even when they pass Keep only if. For example, field source_track_key, starts with, tms- drops tracks OpenTrack itself sent out, read back from a TAK Server."
      >
        <ConditionBuilder label="Drop condition" verb="drop" value={value.drop_if} onChange={(v) => onChange({ ...value, drop_if: v })} fields={fields} />
      </Row>
      {effect && (
        <div className="muted">
          On the stored samples: {effect.dropped ? `drops ${effect.dropped.toLocaleString()}` : 'drops nothing'}
          {effect.examples.length > 0 && <> (e.g. {effect.examples.join(', ')})</>}
        </div>
      )}
    </div>
  )
}

const ALGORITHMS = [{ name: 'gnn' }, { name: 'mht' }, { name: 'plugin' }]
const DOMAINS = ['surface', 'ground', 'air', 'subsurface']

/** Turn detections into tracks. Tracks leave with no identity and unknown affiliation. */
export function TrackerForm({ value, onChange }: Props) {
  const mht = (value.mht as Obj | undefined) ?? {}
  const field = (key: string, label: string, fallback: number, hint: string, obj: Obj = value, set = (v: Obj) => onChange(v)) => (
    <Row label={label} hint={`${hint} Default ${fallback}.`}>
      <Input style={{ ...INPUT, width: 120 }} type="number" aria-label={label} value={String(obj[key] ?? fallback)} onChange={(e) => set({ ...obj, [key]: num(e.target.value) })} />
    </Row>
  )
  const setMht = (m: Obj) => onChange({ ...value, mht: m })
  const auto = (value.auto_timing as Obj | undefined) ?? {}
  const setAuto = (a: Obj) => onChange({ ...value, auto_timing: a })
  const [plugins, setPlugins] = useState<PluginInfo[]>([])
  useEffect(() => {
    if (value.algorithm !== 'plugin') return
    api.plugins().then(setPlugins, () => setPlugins([]))
  }, [value.algorithm])
  const algorithmHint =
    value.algorithm === 'plugin'
      ? 'A tracker plugin (Settings → General → Plugins): its own algorithm and options.'
      : value.algorithm === 'mht'
        ? 'Multiple hypotheses: fewer false tracks in clutter, more work.'
        : 'Global nearest neighbour: the best plot-to-track assignment each scan.'
  const algorithm = (
    <Row label="Algorithm" hint={`${algorithmHint} Tracks get unknown affiliation and at most a domain, never an identity or type.`}>
      <FieldSelect
        ariaLabel="Tracker algorithm"
        fields={ALGORITHMS}
        value={String(value.algorithm ?? 'gnn')}
        onChange={(a) => onChange(a === 'plugin' ? { algorithm: 'plugin', plugin: undefined, options: {} } : { ...value, algorithm: a ?? 'gnn', plugin: undefined, options: undefined })}
        style={{ width: 160 }}
      />
    </Row>
  )
  const profiles = (
    <TrackerProfiles
      value={value}
      onChange={onChange}
      row={(label, hint, control) => (
        <Row label={label} hint={hint}>
          {control}
        </Row>
      )}
    />
  )
  if (value.algorithm === 'plugin') {
    const plugin = plugins.find((p) => p.name === value.plugin)
    return (
      <div className="stack">
        {profiles}
        {algorithm}
        <Row label="Plugin" hint="The enabled plugins that provide a tracker.">
          <PluginPicker
            plugins={plugins}
            kind="tracker"
            value={value.plugin as string | undefined}
            onChange={(p) => onChange({ algorithm: 'plugin', plugin: p.name, options: structuredClone(p.default_options) })}
          />
        </Row>
        {plugin?.description && <p className="muted small">{plugin.description}</p>}
        <PluginOptions
          plugin={plugin}
          value={(value.options as Obj | undefined) ?? {}}
          onChange={(options) => onChange({ ...value, options })}
          row={(key, label, help, control) => (
            <Row key={key} label={label} hint={help}>
              {control}
            </Row>
          )}
        />
      </div>
    )
  }
  return (
    <div className="stack">
      {profiles}
      {algorithm}
      <Row label="Domain" hint="Everything this sensor sees. Empty: what most of a track's plots report, if any.">
        <FieldSelect ariaLabel="Tracker domain" allowNone fields={DOMAINS.map((name) => ({ name }))} value={(value.domain as string) ?? null} onChange={(d) => onChange({ ...value, domain: d ?? undefined })} style={{ width: 160 }} />
      </Row>
      {field('measurement_sigma_m', 'Plot error σ (m)', 10, 'Position error of a plot, one standard deviation per axis, for detections that report no error ellipse or circular error of their own.')}
      {field('process_noise_mps2', 'Manoeuvre (m/s²)', 0.5, 'How hard targets accelerate or turn (the filter\'s process noise). Higher follows manoeuvres better but gives noisier tracks.')}
      {field('cluster_m', 'Merge plots within (m)', 0, 'Plots of one scan closer than this are merged into one at their centroid, for sensors that return several points off one large object (lidar on a ship). 0: off.')}
      <h4 className="subhead">
        Existence
        <InfoTip label="Existence">
          Each track carries the probability that it is a real target: plots raise it by how well they fit against clutter, looks without one lower
          it. It is published as the track&apos;s confidence.
        </InfoTip>
      </h4>
      {field('detection_probability', 'Detection probability', 0.9, 'Chance the sensor detects a target it looks at, between 0 and 1. A look without a plot lowers existence more when this is high.')}
      {field('clutter_density', 'False plots per m²', 1e-6, 'Expected false plots per square metre per look. Higher makes a plot count for less, so tracks confirm more slowly.')}
      {field('birth_density', 'New targets per m²', 1e-7, 'Expected new targets per square metre per look. A new track starts at existence birth / (birth + clutter).')}
      {field('confirm_probability', 'Confirm at probability', 0.95, 'A track is confirmed, and reported, once its existence reaches this and it has enough plots. Must be above the drop probability and below 1.')}
      {field('drop_probability', 'Drop at probability', 0.02, 'A track is dropped once its existence falls to this. Must be above 0.')}
      {field('target_lifetime_secs', 'Target lifetime (s)', 600, 'How long a target lasts on average: existence decays at this rate between looks, so misses can end even a long-lived track.')}
      {field('confirm_hits', 'Confirm after at least (plots)', 3, 'Fewest plots a track needs before it can be confirmed, whatever its existence. At least 1.')}
      <Row label="Auto timing" hint="Size the drop windows to the sensor's revisit rate, as the codec measures it (STANAG 4607), and count a miss once per revisit instead of once per scan. Each window is the revisits below times the period, but never shorter than its floor. Until the period is known, the fixed drop times apply.">
        <Toggle
          size="sm"
          aria-label="Auto timing"
          value={value.auto_timing != null}
          onChange={(on) => onChange({ ...value, auto_timing: on ? {} : undefined })}
        />
      </Row>
      {value.auto_timing != null ? (
        <>
          {field('drop_tentative_revisits', 'Drop unconfirmed after (revisits)', 1.3, 'An unconfirmed track is dropped this many revisit periods after its last plot. Above 1, or none survives to the next revisit.', auto, setAuto)}
          {field('drop_confirmed_revisits', 'Drop after (revisits)', 2.5, 'A confirmed track is dropped this many revisit periods after its last plot. Above 1.', auto, setAuto)}
          {field('min_drop_tentative_secs', '…drop unconfirmed after at least (s)', 8, 'Floor on the unconfirmed drop window, for sensors that revisit every few seconds but still lose a target for longer (terrain, a stop).', auto, setAuto)}
          {field('min_drop_confirmed_secs', '…drop after at least (s)', 30, 'Floor on the confirmed drop window.', auto, setAuto)}
        </>
      ) : (
        <>
          {field('drop_tentative_secs', 'Drop unconfirmed after (s) without a plot', 3, 'An unconfirmed track with no plot for this long is dropped.')}
          {field('drop_confirmed_secs', 'Drop after (s) without a plot', 8, 'A confirmed track with no plot for this long is dropped (reported once as dropped).')}
        </>
      )}
      <Row label="Scans" hint="A scan is one frame, or the plots with the same time.">
        <FieldSelect
          ariaLabel="Scan grouping"
          fields={[{ name: 'frame' }, { name: 'time' }]}
          value={String(value.scans ?? 'frame')}
          onChange={(v) => onChange({ ...value, scans: v === 'time' ? 'time' : undefined })}
          style={{ width: 160 }}
        />
      </Row>
      <Row label="Track keys" hint="Prefix of the keys the tracker gives its tracks: prefix, a tag for the tracker's run, and a number (T3f2a-12). The run tag keeps a restarted tracker from continuing an earlier run's tracks. Default T.">
        <Input style={{ ...INPUT, width: 120 }} aria-label="Track key prefix" value={String(value.key_prefix ?? 'T')} onChange={(e) => onChange({ ...value, key_prefix: e.target.value || undefined })} spellCheck={false} />
      </Row>
      {value.algorithm === 'mht' && (
        <>
          <h4 className="subhead">
            Hypotheses
            <InfoTip label="Hypotheses">
              MHT keeps competing plot-to-track assignments for a few scans and settles on the most likely, instead of committing each scan as GNN does.
            </InfoTip>
          </h4>
          {field('n_scan', 'Final after scans', 3, 'Scans an association stays open to revision before it is final. More resolves crossing targets better, at more work and later output.', mht, setMht)}
          {field('max_branches', 'Hypotheses per target', 20, 'Competing hypotheses kept per possible target; the least likely beyond this are pruned.', mht, setMht)}
        </>
      )}
    </div>
  )
}

/** What the source's observations are, which decides how correlation takes them. */
export function PublishForm({
  reports,
  tracker,
  publishAlone,
  confirmAfter,
  onChange,
  onPublishAlone,
  onConfirmAfter,
}: {
  reports?: string
  tracker: boolean
  publishAlone?: boolean
  onChange: (r: 'detections' | undefined) => void
  onPublishAlone: (v: boolean | undefined) => void
  confirmAfter?: number
  onConfirmAfter: (v: number | undefined) => void
}) {
  const alone = publishAlone ?? reports !== 'detections'
  return (
    <div className="stack">
      <Row
        label="The feed reports"
        hint={
          tracker
            ? 'The tracker stage turns the plots into tracks; correlation pairs them with other sources.'
            : reports === 'detections'
              ? 'Each plot updates the nearest system track another source keeps; plots near none are dropped. Add a Tracker stage to form tracks instead.'
              : 'Every observation that passes reaches correlation.'
        }
      >
        <FieldSelect
          ariaLabel="The feed reports"
          fields={[{ name: 'tracks' }, { name: 'detections' }]}
          value={reports ?? 'tracks'}
          onChange={(v) => onChange(v === 'detections' ? 'detections' : undefined)}
          style={{ width: 160 }}
        />
      </Row>
      <Row label="Publish its lone tracks" hint="No: its tracks stay inside OpenTrack until a source that may stand alone reports for them too.">
        <FieldSelect
          ariaLabel="Publish lone tracks"
          fields={[{ name: 'yes' }, { name: 'no' }]}
          value={alone ? 'yes' : 'no'}
          onChange={(v) => {
            const want = v === 'yes'
            onPublishAlone(want === (reports !== 'detections') ? undefined : want)
          }}
          style={{ width: 160 }}
        />
      </Row>
      <Row
        label="Confirm after"
        hint={`Reports a new track needs from this source before it is confirmed and can be published. Empty: ${reports === 'detections' && !tracker ? "the engine's default (3), since one plot may be clutter" : '1, since a feed\'s track is already a track'}. A track's entity set to always publish skips this.`}
      >
        <div className="num-row">
          <Input
            style={{ ...INPUT, width: 90 }}
            type="number"
            min={1}
            step={1}
            aria-label="Confirm after reports"
            value={confirmAfter === undefined ? '' : String(confirmAfter)}
            placeholder={reports === 'detections' && !tracker ? '3' : '1'}
            onChange={(e) => onConfirmAfter(e.target.value.trim() === '' ? undefined : Math.max(1, Math.round(Number(e.target.value))))}
          />
          <span className="muted">reports</span>
        </div>
      </Row>
    </div>
  )
}

/** Defaults the engine uses when a source sets no emitter motion: surface traffic. */
const MOTION_DEFAULT: EmitterMotion = { manoeuvre_mps2: 0.1, max_speed_mps: 30 }

/** For a source reporting lines of bearing: how the emitters it hears may move. */
export function EmitterMotionForm({ value, onChange }: { value?: EmitterMotion; onChange: (v: EmitterMotion | undefined) => void }) {
  const set = (key: keyof EmitterMotion, raw: string) => {
    const next = { ...(value ?? MOTION_DEFAULT) }
    const n = Number(raw)
    next[key] = raw.trim() === '' || !Number.isFinite(n) ? MOTION_DEFAULT[key] : Math.max(0, n)
    const isDefault = next.manoeuvre_mps2 === MOTION_DEFAULT.manoeuvre_mps2 && next.max_speed_mps === MOTION_DEFAULT.max_speed_mps
    onChange(isDefault ? undefined : next)
  }
  return (
    <div className="stack">
      <Row
        label="Emitters' manoeuvre"
        hint="For a source reporting lines of bearing, located from one moving sensor: the hardest the emitters it hears may turn or weave, which a steady-course fit cannot see. It widens the stated error. A ship's turn is about 0.1 m/s²; a small fast boat's weave 0.2-0.3; an aircraft's turn several. Empty: 0.1."
      >
        <div className="num-row">
          <Input
            style={{ ...INPUT, width: 90 }}
            type="number"
            min={0}
            step={0.05}
            aria-label="Emitters' manoeuvre, metres per second squared"
            value={value ? String(value.manoeuvre_mps2) : ''}
            placeholder={String(MOTION_DEFAULT.manoeuvre_mps2)}
            onChange={(e) => set('manoeuvre_mps2', e.target.value)}
          />
          <span className="muted">m/s²</span>
        </div>
      </Row>
      <Row
        label="Emitters' top speed"
        hint="The fastest the emitters it hears may move. An emitter heading straight along the line of sight barely turns its bearings, so it can look stationary; the stated error then allows for it having moved at up to this speed. Empty: 30 m/s (about 60 knots)."
      >
        <div className="num-row">
          <Input
            style={{ ...INPUT, width: 90 }}
            type="number"
            min={0}
            step={1}
            aria-label="Emitters' top speed, metres per second"
            value={value ? String(value.max_speed_mps) : ''}
            placeholder={String(MOTION_DEFAULT.max_speed_mps)}
            onChange={(e) => set('max_speed_mps', e.target.value)}
          />
          <span className="muted">m/s</span>
        </div>
      </Row>
    </div>
  )
}

export function ThrottleForm({ value, onChange }: Props) {
  const field = (key: string, label: string, hint: string, fallback: number) => (
    <Row label={label} hint={hint}>
      <Input style={{ ...INPUT, width: 120 }} type="number" aria-label={label} value={String(value[key] ?? fallback)} onChange={(e) => onChange({ ...value, [key]: num(e.target.value) })} />
    </Row>
  )
  return (
    <div className="stack">
      {field('min_interval_secs', 'At most every (s)', 'Never more often than this per source track.', 0)}
      {field('min_move_m', 'Or after moving (m)', 'Between that and the heartbeat, only when the track moved this far.', 0)}
      {field('heartbeat_secs', 'Heartbeat (s)', 'Always pass a report after this long.', 600)}
    </div>
  )
}

/** An optional security label on everything the source reports, in the fields OpenStare's ICD
 *  reserves (`security.classification`, `security.restrictions`, `security.sharing`). Free text. */
export function SecurityForm({ value, onChange }: { value?: SecurityLabel; onChange: (v: SecurityLabel | undefined) => void }) {
  const on = value !== undefined
  const v = value ?? { classification: '' }
  const set = (patch: Partial<SecurityLabel>) => onChange({ ...v, ...patch })
  return (
    <div className="stack">
      <h4 className="subhead">
        Security label
        <InfoTip label="Security label">
          Label everything this source reports: its tracks carry it into OpenTrack&apos;s output as <span className="mono">security</span>. A track
          several labelled sources report for takes the label of the highest-priority one.
        </InfoTip>
      </h4>
      <Row label="Label this source" hint="On: every track this source reports carries the label below. Off: no label from this source.">
        <Toggle size="sm" aria-label="Label this source" value={on} onChange={(yes) => onChange(yes ? { classification: '' } : undefined)} />
      </Row>
      {on && (
        <>
          <Row label="Classification" hint="security.classification, e.g. SECRET">
            <Input style={{ ...INPUT, width: 320 }} aria-label="Classification" value={v.classification} maxLength={256} onChange={(e) => set({ classification: e.target.value })} />
          </Row>
          <Row label="Restrictions" hint="security.restrictions: comma separated, e.g. NOFORN, ORCON">
            <Input
              style={{ ...INPUT, width: 320 }}
              aria-label="Restrictions"
              defaultValue={(v.restrictions ?? []).join(', ')}
              onBlur={(e) => {
                const r = e.target.value.split(',').map((x) => x.trim()).filter(Boolean)
                set({ restrictions: r.length ? r : undefined })
              }}
            />
          </Row>
          <Row label="Sharing" hint="security.sharing: who the classification may be released to, e.g. REL TO USA, FVEY">
            <Input style={{ ...INPUT, width: 320 }} aria-label="Sharing" value={v.sharing ?? ''} maxLength={256} onChange={(e) => set({ sharing: e.target.value || undefined })} />
          </Row>
        </>
      )}
    </div>
  )
}
