/**
 * Plain-language views of a source pipeline, for the Pipeline flowchart and field map.
 *
 * Mirrors the server's evaluation (ot-source `expr.rs`, `mapping.rs`, `pipeline.rs`): a value spec
 * is a path string or an object with one source (`path`, `const`, `first`, `template`, `cases`,
 * `arith`) followed by `null_if`, `transforms`, `table`, `ranges`, `range` and `default`.
 */
import type { SchemaOverview, SourceSpec } from '../api/client'

type Obj = Record<string, unknown>
export type ValueSpec = string | Obj

const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v)
const lit = (v: unknown) => (typeof v === 'string' ? `"${v}"` : JSON.stringify(v))
const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`

/** Every record path a value spec reads. */
export function valueSources(spec: unknown): string[] {
  if (typeof spec === 'string') return [spec]
  if (!isObj(spec)) return []
  const out: string[] = []
  if (typeof spec.path === 'string') out.push(spec.path)
  if (spec.key !== undefined) out.push(...valueSources(spec.key))
  for (const s of (spec.first as unknown[]) ?? []) out.push(...valueSources(s))
  for (const s of (spec.args as unknown[]) ?? []) out.push(...valueSources(s))
  if (typeof spec.template === 'string') {
    for (const m of spec.template.matchAll(/\{([^}]+)\}/g)) out.push(m[1])
  }
  for (const c of (spec.cases as Obj[]) ?? []) {
    out.push(...conditionSources(c.when), ...valueSources(c.then))
  }
  return [...new Set(out)]
}

function conditionSources(c: unknown): string[] {
  if (!isObj(c)) return []
  if (Array.isArray(c.all)) return c.all.flatMap(conditionSources)
  if (Array.isArray(c.any)) return c.any.flatMap(conditionSources)
  if (c.not !== undefined) return conditionSources(c.not)
  return [...(typeof c.path === 'string' ? [c.path] : []), ...valueSources(c.value)]
}

const OPS: [string, string][] = [
  ['eq', '='],
  ['ne', '≠'],
  ['in', 'in'],
  ['not_in', 'not in'],
  ['gt', '>'],
  ['gte', '≥'],
  ['lt', '<'],
  ['lte', '≤'],
  ['matches', 'matches'],
  ['starts_with', 'starts with'],
  ['contains', 'contains'],
  ['bits_any', 'has any bits of'],
]

/** A condition as a sentence fragment: `MessageType in ["A", "B"] and not (flight matches "^TEST")`. */
export function describeCondition(c: unknown): string {
  if (!isObj(c)) return String(c)
  if (Array.isArray(c.all)) return c.all.map(describeCondition).join(' and ')
  if (Array.isArray(c.any)) return `(${c.any.map(describeCondition).join(' or ')})`
  if (c.not !== undefined) return `not (${describeCondition(c.not)})`
  const subject = typeof c.path === 'string' ? c.path : describeValue(c.value as ValueSpec)
  const parts: string[] = []
  if (c.exists !== undefined) parts.push(c.exists ? 'is present' : 'is missing')
  for (const [k, word] of OPS) {
    if (c[k] !== undefined) {
      const v = c[k]
      const shown = Array.isArray(v) && v.length > 6 ? `${plural(v.length, 'value')}` : lit(v)
      parts.push(`${word} ${shown}`)
    }
  }
  return `${subject} ${parts.join(' and ')}`.trim()
}

/** A value spec in a few words: where the value comes from and what is done to it. */
export function describeValue(spec: ValueSpec | undefined): string {
  if (spec === undefined) return '—'
  if (typeof spec === 'string') return spec || 'the whole record'
  const bits: string[] = []
  if (spec.const !== undefined) bits.push(`${lit(spec.const)} (fixed)`)
  if (typeof spec.path === 'string') bits.push(spec.key !== undefined ? `${spec.path}[${describeValue(spec.key as ValueSpec)}]` : spec.path)
  if (Array.isArray(spec.first)) bits.push(`first of ${spec.first.map((s) => describeValue(s as ValueSpec)).join(', ')}`)
  if (typeof spec.template === 'string') bits.push(`template ${lit(spec.template)}`)
  if (Array.isArray(spec.cases)) bits.push(`${plural(spec.cases.length, 'case')}`)
  if (typeof spec.arith === 'string') {
    bits.push(`${spec.arith}(${((spec.args as ValueSpec[]) ?? []).map(describeValue).join(', ')})`)
  }
  const after: string[] = []
  if (Array.isArray(spec.null_if) && spec.null_if.length) after.push(`ignore ${spec.null_if.map(lit).join(', ')}`)
  if (Array.isArray(spec.transforms) && spec.transforms.length) after.push(spec.transforms.join(', '))
  if (isObj(spec.table)) {
    after.push(`look up in a ${plural(Object.keys(spec.table).length, 'entry', 'entries')} table`)
    if (spec.table_default !== undefined) after.push(`else ${lit(spec.table_default)}`)
  }
  if (Array.isArray(spec.ranges) && spec.ranges.length) after.push(`${plural(spec.ranges.length, 'range')} lookup`)
  if (Array.isArray(spec.range)) after.push(`only ${spec.range[0]}–${spec.range[1]}`)
  if (spec.default !== undefined) after.push(`default ${lit(spec.default)}`)
  return [bits.join(' '), ...after].filter(Boolean).join(' · ')
}

// --- Stages --------------------------------------------------------------------------------

export interface Fact {
  label: string
  value: string
}

export interface Stage {
  id: string
  title: string
  /** One line under the title in the flowchart. */
  summary: string
  facts: Fact[]
}

const secs = (s: number) => (s >= 86400 ? `${+(s / 86400).toFixed(1)} d` : s >= 3600 ? `${+(s / 3600).toFixed(1)} h` : s >= 60 ? `${+(s / 60).toFixed(1)} min` : `${s} s`)

function transportStage(spec: SourceSpec): Stage {
  const t = spec.transport
  const where = String(t.url ?? t.bind ?? (t.host ? `${t.host}:${t.port ?? ''}` : ''))
  const facts: Fact[] = Object.entries(t)
    .filter(([k]) => k !== 'type')
    .map(([k, v]) => ({ label: k, value: typeof v === 'string' ? v : JSON.stringify(v) }))
  return { id: 'transport', title: t.type.replace('_', ' '), summary: where, facts }
}

/** The stages a record passes through, in the order the source worker runs them. */
export function pipelineStages(spec: SourceSpec): Stage[] {
  const p = spec.pipeline as SourceSpec['pipeline'] & Obj
  const codec = p.codec
  const rules = (p.mapping.rules as Obj[]) ?? []
  const reject = (p.mapping.reject as Obj[]) ?? []
  const fieldCount = rules.reduce((n, r) => n + Object.keys((r.fields as Obj) ?? {}).length, 0)
  const out: Stage[] = [
    transportStage(spec),
    {
      id: 'decode',
      title: `decode ${codec.type}`,
      summary: typeof codec.records === 'string' ? `records at "${codec.records}"` : 'one record per frame',
      facts: Object.entries(codec)
        .filter(([k]) => k !== 'type')
        .map(([k, v]) => ({ label: k, value: typeof v === 'string' ? v : JSON.stringify(v) })),
    },
  ]
  if (reject.length) {
    out.push({
      id: 'reject',
      title: 'reject',
      summary: plural(reject.length, 'rule'),
      facts: reject.map((r) => ({ label: String(r.reason), value: describeCondition(r.when) })),
    })
  }
  out.push({
    id: 'map',
    title: 'map',
    summary: `${plural(rules.length, 'rule')} · ${plural(fieldCount, 'field')}`,
    facts: [],
  })
  if (rules.some((r) => r.kind === 'static')) {
    const ttl = Number((p.static_join as Obj | undefined)?.ttl_secs ?? 7 * 86400)
    out.push({
      id: 'join',
      title: 'identity join',
      summary: `keeps identity fields ${secs(ttl)}`,
      facts: [
        { label: 'What it does', value: 'Fields from static rules are cached per track key and fill gaps in later position reports.' },
        { label: 'Kept for', value: secs(ttl) },
      ],
    })
  }
  const reg = p.registry as Obj | undefined
  const links = entityLinks(reg)
  const toTrack = links.filter((l) => l.direction !== 'to_entity').length
  out.push({
    id: 'registry',
    title: 'entity',
    summary: `${plural(toTrack, 'field')} from the entity${links.length > toTrack ? `, ${links.length - toTrack} back to it` : ''}${reg ? '' : ' (defaults)'}`,
    facts: [
      { label: 'Schemes', value: ((reg?.schemes as string[]) ?? []).join(', ') || 'every identifier' },
      { label: 'Name to grade', value: String(reg?.broadcast_name ?? 'name') },
      { label: 'Used at grades', value: ((reg?.apply_grades as string[]) ?? DEFAULT_GRADES).join(', ') },
      ...links.map((l) =>
        l.direction === 'to_entity'
          ? { label: `updates entity ${l.entity}`, value: `from the track's ${l.track}` }
          : { label: `sets ${l.track}`, value: `from the entity's ${l.entity}` },
      ),
    ],
  })
  const aff = p.affiliation as Obj | undefined
  if (aff) {
    const n = (k: string) => ((aff[k] as string[]) ?? []).length
    out.push({
      id: 'affiliation',
      title: 'affiliation',
      summary: `by country · ${n('friendly')} friendly, ${n('hostile')} hostile`,
      facts: [
        { label: 'Country from', value: describeValue(aff.country as ValueSpec) },
        { label: 'Friend', value: ((aff.friendly as string[]) ?? []).join(' ') || '—' },
        { label: 'Hostile', value: ((aff.hostile as string[]) ?? []).join(' ') || '—' },
        { label: 'Neutral', value: ((aff.neutral as string[]) ?? []).join(' ') || '—' },
        { label: 'Other countries', value: String(aff.otherwise ?? 'left unset') },
        { label: 'No country', value: String(aff.unknown_country ?? 'left unset') },
      ],
    })
  }
  const filter = p.filter as Obj | undefined
  if (filter) {
    const facts: Fact[] = []
    if (filter.keep_if) facts.push({ label: 'Keep only if', value: describeCondition(filter.keep_if) })
    if (filter.drop_if) facts.push({ label: 'Drop if', value: describeCondition(filter.drop_if) })
    out.push({ id: 'filter', title: 'filter', summary: filter.keep_if ? 'keep only matching' : 'drop matching', facts })
  }
  const tr = p.tracker as Obj | undefined
  if (tr) {
    const mht = (tr.mht as Obj | undefined) ?? {}
    const alg = tr.algorithm === 'mht' ? 'MHT' : tr.algorithm === 'plugin' ? `plugin ${String(tr.plugin ?? '')}` : 'GNN'
    const profile: Fact[] = tr.profile ? [{ label: 'Profile', value: String(tr.profile) }] : []
    const facts: Fact[] =
      tr.algorithm === 'plugin'
        ? [
            ...profile,
            { label: 'Tracker', value: `the ${String(tr.plugin ?? '?')} plugin (Settings → General → Plugins)` },
            { label: 'Options', value: Object.keys((tr.options as Obj | undefined) ?? {}).length ? JSON.stringify(tr.options) : 'its defaults' },
            { label: 'Track keys', value: 'as the plugin names them' },
          ]
        : [
            ...profile,
            { label: 'Association', value: alg === 'MHT' ? `multiple hypotheses, final after ${Number(mht.n_scan ?? 3)} scans` : 'global nearest neighbour' },
            { label: 'Classification', value: `unknown affiliation${tr.domain ? `, ${String(tr.domain)}` : ', domain from the plots if they report one'}; no identity` },
            {
              label: 'Confirmed at',
              value: `existence ${Number(tr.confirm_probability ?? 0.95)}, after at least ${Number(tr.confirm_hits ?? 3)} plots`,
            },
            { label: 'Clutter', value: `${Number(tr.clutter_density ?? 1e-6)} false plots per m², detection ${Number(tr.detection_probability ?? 0.9)}` },
            {
              label: 'Dropped after',
              value: tr.auto_timing != null ? 'a few missed revisits (auto timing)' : `${secs(Number(tr.drop_confirmed_secs ?? 8))} without a plot`,
            },
            { label: 'Track keys', value: `${String(tr.key_prefix ?? 'T')}<run>-1, -2…` },
          ]
    out.push({
      id: 'tracker',
      title: `tracker · ${alg}`,
      summary: `plots → tracks · ${tr.domain ? `${String(tr.domain)}, ` : ''}${tr.profile ? `${String(tr.profile)} · ` : ''}unknown affiliation`,
      facts,
    })
  }
  const th = p.throttle as Obj | undefined
  if (th) {
    const min = Number(th.min_interval_secs ?? 0)
    const beat = Number(th.heartbeat_secs ?? 600)
    const move = Number(th.min_move_m ?? 0)
    out.push({
      id: 'throttle',
      title: 'throttle',
      summary: `≥ ${secs(min)} apart · ${move} m move`,
      facts: [
        { label: 'At most every', value: secs(min) },
        { label: 'Between that and the heartbeat', value: move > 0 ? `only after moving ${move} m` : 'every report' },
        { label: 'Heartbeat', value: `always after ${secs(beat)}` },
      ],
    })
  }
  const plots = spec.reports === 'detections' && !tr
  out.push({
    id: 'publish',
    title: 'correlation → TRACKS',
    summary: plots ? 'plots associated with system tracks' : 'system track, published to NATS',
    facts: [
      {
        label: 'Correlation',
        value: plots
          ? 'Each plot updates the nearest system track inside the gate; plots with none are dropped.'
          : 'Each source track reports for one system track, paired with other sources’ tracks on a shared identifier or kinematic agreement.',
      },
      {
        label: 'Published',
        value: 'Confirmed tracks that a source allowed to stand alone reports for: the GOLD fields plus the output schema’s attributes.',
      },
    ],
  })
  return out
}

// --- Decode stage ------------------------------------------------------------------------

/** The codecs the Decode stage edits; the others (protobuf, plugins) are chosen on the Transport tab. */
export const DECODE_CODECS = ['json', 'cot_xml', 'xml']

/**
 * A codec the Decode stage only shows: its type and its message or plugin, or null for one it
 * edits. Choosing a Decode format would replace such a codec's schema, plugin and options.
 */
export function transportCodec(codec: Obj | undefined): { type: string; name?: string } | null {
  const type = String(codec?.type ?? 'json')
  if (DECODE_CODECS.includes(type)) return null
  const name = type === 'protobuf' ? codec?.message : type === 'plugin' ? codec?.plugin : undefined
  return typeof name === 'string' && name ? { type, name } : { type }
}

// --- Entity links --------------------------------------------------------------------------

export interface EntityLink {
  entity: string
  track: string
  direction?: 'to_track' | 'to_entity'
}

export const DEFAULT_GRADES = ['exact', 'hull', 'name', 'generic']

/** The entity's OTH-GOLD minimum populating the track: the links a stage has unless it lists its own. */
export const DEFAULT_LINKS: EntityLink[] = [
  { entity: 'name', track: 'name' },
  { entity: 'class_name', track: 'platform.class' },
  { entity: 'domain', track: 'classification.domain' },
  { entity: 'affiliation', track: 'classification.affiliation' },
  { entity: 'track_type', track: 'track_type' },
  { entity: 'cot_type', track: 'classification.cot_type' },
  { entity: 'sidc', track: 'classification.sidc' },
].map((l) => ({ ...l, direction: 'to_track' as const }))

/** A stage's links: its own, an earlier form's `apply` (track field ← entity field), or the defaults. */
export function entityLinks(reg: Obj | undefined): EntityLink[] {
  if (Array.isArray(reg?.links)) return reg.links as EntityLink[]
  const apply = Object.entries((reg?.apply as Obj) ?? {})
  if (apply.length) {
    const renamed: Record<string, string> = { cot: 'cot_type', ship_class: 'class_name' }
    return apply.map(([track, from]) => ({ entity: renamed[String(from)] ?? String(from), track, direction: 'to_track' }))
  }
  return DEFAULT_LINKS
}

// --- Field map -----------------------------------------------------------------------------

export type Destination = { text: string; kind: 'attribute' | 'gold' | 'internal' | 'none' }

export interface FieldRow {
  id: string
  /** Where the value is set: a mapping rule, the entity or the affiliation stage. */
  stage: string
  from: string
  how: string
  target: string
  published: Destination[]
}

/** The latest published output schema's fields. */
function outputFields(schema: SchemaOverview) {
  return schema.versions.find((v) => v.version === schema.latest_published)?.fields ?? []
}

/** Where a value written to `target` ends up in the published message. */
export function destinations(target: string, schema: SchemaOverview): Destination[] {
  const fields = outputFields(schema)
  if (target.startsWith('entity.')) return [{ kind: 'internal', text: `updates the entity's ${target.slice(7)}` }]
  if (target.startsWith('ext.')) {
    const key = target.slice(4)
    const f = fields.find((x) => x.key === key)
    if (!f) return [{ kind: 'none', text: `not published: the output schema has no field "${key}"` }]
    if (f.builtin) return [{ kind: 'none', text: `not published: "${key}" is filled by the ${f.builtin} built-in instead` }]
    return [{ kind: 'attribute', text: `attributes.${key}` }]
  }
  const out: Destination[] = []
  for (const [gold, from] of Object.entries(schema.gold_sources ?? {})) {
    if (from.includes(target)) out.push({ kind: 'gold', text: gold })
  }
  const readers = (schema.builtins ?? []).filter((b) => (b.reads ?? []).includes(target))
  for (const b of readers) {
    for (const f of fields.filter((x) => x.builtin === b.name)) out.push({ kind: 'attribute', text: `attributes.${f.key}` })
  }
  if (!out.some((d) => d.kind === 'attribute')) {
    if (readers.length && !out.length) {
      out.push({
        kind: 'none',
        text: `not published: no schema field is linked to the ${readers.map((b) => b.name).join(' / ')} built-in`,
      })
    } else if (!out.length) {
      out.push({ kind: 'internal', text: 'used inside OpenTrack only' })
    }
  }
  return out
}

/** Every value a pipeline sets, traced from the feed to the published message. */
export function fieldMap(spec: SourceSpec, schema: SchemaOverview): FieldRow[] {
  const p = spec.pipeline as SourceSpec['pipeline'] & Obj
  const rows: FieldRow[] = []
  for (const [i, r] of ((p.mapping.rules as Obj[]) ?? []).entries()) {
    const stage = `${String(r.name ?? `rule ${i + 1}`)}${r.kind === 'static' ? ' (identity)' : ''}`
    rows.push({
      id: `${i}:key`,
      stage,
      from: valueSources(r.key).join(', '),
      how: describeValue(r.key as ValueSpec),
      target: 'track key',
      published: [{ kind: 'internal', text: 'identifies the source track' }],
    })
    for (const [j, id] of ((r.identifiers as Obj[]) ?? []).entries()) {
      rows.push({
        id: `${i}:id${j}`,
        stage,
        from: valueSources(id.value).join(', '),
        how: describeValue(id.value as ValueSpec),
        target: `identifier ${String(id.scheme)}`,
        published: destinations('identifiers', schema).map((d) =>
          d.kind === 'internal' ? { kind: 'internal', text: 'registry matching and correlation' } : d,
        ),
      })
    }
    for (const [target, v] of Object.entries((r.fields as Obj) ?? {})) {
      rows.push({
        id: `${i}:${target}`,
        stage,
        from: valueSources(v).join(', ') || '—',
        how: describeValue(v as ValueSpec),
        target,
        published: destinations(target, schema),
      })
    }
  }
  for (const [n, l] of entityLinks(p.registry as Obj | undefined).entries()) {
    rows.push(
      l.direction === 'to_entity'
        ? {
            id: `registry:${n}`,
            stage: 'entity',
            from: l.track,
            how: 'on a corroborated match, updates the entity',
            target: `entity ${l.entity}`,
            published: [{ kind: 'internal', text: `the entity's ${l.entity}` }],
          }
        : {
            id: `registry:${n}`,
            stage: 'entity',
            from: `entity ${l.entity}`,
            how: 'on a corroborated match, replaces the feed',
            target: l.track,
            published: destinations(l.track, schema),
          },
    )
  }
  const aff = p.affiliation as Obj | undefined
  if (aff) {
    rows.push({
      id: 'affiliation',
      stage: 'affiliation',
      from: valueSources(aff.country).join(', '),
      how: 'country lists',
      target: 'classification.affiliation',
      published: destinations('classification.affiliation', schema),
    })
  }
  return rows
}
