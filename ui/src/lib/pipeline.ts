/**
 * Plain-language views of a source's settings, for its steps and the field map.
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
  // Every Map's rules, and the first Entity links step's (a source from before flows has one of each).
  const nodes = (p.nodes as { type?: string; config?: Obj }[] | undefined) ?? []
  const rules = nodes.length
    ? nodes.filter((n) => n.type === 'map').flatMap((n) => ((n.config?.mapping as Obj | undefined)?.rules as Obj[]) ?? [])
    : (((p.mapping as Obj | undefined)?.rules as Obj[]) ?? [])
  const registry = nodes.length ? nodes.find((n) => n.type === 'registry')?.config : (p.registry as Obj | undefined)
  for (const [i, r] of rules.entries()) {
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
  for (const [n, l] of entityLinks(registry).entries()) {
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
