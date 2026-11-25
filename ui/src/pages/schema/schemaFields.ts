import type { ExtensionField, SchemaOverview, SourceRow } from '../../api/client'
import { fieldMap, type FieldRow as Mapping } from '../../lib/pipeline'

export const FIELD_TYPES: ExtensionField['type'][] = ['string', 'integer', 'number', 'boolean', 'enum', 'timestamp', 'position', 'json']
export const BACKGROUND_TABLE = { border: 0, borderRadius: 0 } as const

const CORE_TYPES: Record<string, ExtensionField['type'] | undefined> = {
  track_id: 'string', class: 'string', name: 'string', domain: 'string', affiliation: 'string',
  force_code: 'integer', track_type: 'string', sidc: 'json', time: 'timestamp', lat: 'number', lon: 'number',
}

export interface FieldInput extends Mapping {
  source: string
  version: number
}

export interface SchemaFieldRow {
  id: string
  key: string
  core: boolean
  type?: ExtensionField['type']
  unit?: string
  field?: ExtensionField
  inputs: FieldInput[]
  targets: string[]
}

export function schemaFields(overview: SchemaOverview, fields: ExtensionField[], sources: SourceRow[]): SchemaFieldRow[] {
  const view: SchemaOverview = {
    ...overview,
    latest_published: 0,
    versions: [{ version: 0, status: 'published', published_at_ms: null, notes: null, fields }],
  }
  const versions = new Map(overview.sources.map(({ source, schema_version }) => [source, schema_version]))
  const inputs = new Map<string, FieldInput[]>()
  for (const source of sources) {
    for (const mapping of fieldMap(source.spec, view)) {
      for (const destination of mapping.published) {
        if (destination.kind !== 'gold' && destination.kind !== 'attribute') continue
        const list = inputs.get(destination.text) ?? []
        list.push({ ...mapping, source: source.id, version: versions.get(source.id) ?? 1 })
        inputs.set(destination.text, list)
      }
    }
  }
  return [
    ...overview.published_core.map((key) => ({
      id: key, key, core: true, type: CORE_TYPES[key], unit: key === 'lat' || key === 'lon' ? 'degrees' : undefined,
      inputs: inputs.get(key) ?? [], targets: overview.gold_sources?.[key] ?? [],
    })),
    ...fields.map((field) => {
      const id = `attributes.${field.key}`
      const builtin = overview.builtins.find((b) => b.name === field.builtin)
      return {
        id, key: field.key, field, core: false, type: field.type, unit: field.unit,
        inputs: inputs.get(id) ?? [],
        targets: builtin ? builtin.reads ?? [] : [`ext.${field.key}`],
      }
    }),
  ]
}

function stableJson(value: unknown): string {
  return JSON.stringify(value, (_, part: unknown) => (
    part && typeof part === 'object' && !Array.isArray(part)
      ? Object.fromEntries(Object.entries(part).sort(([a], [b]) => a.localeCompare(b)))
      : part
  ))
}

export const sameFields = (a: ExtensionField[], b: ExtensionField[]) =>
  stableJson([...a].sort((x, y) => x.key.localeCompare(y.key))) === stableJson([...b].sort((x, y) => x.key.localeCompare(y.key)))

export interface SchemaChange {
  before?: SchemaFieldRow
  after?: SchemaFieldRow
  kind: 'Added' | 'Removed' | 'Changed' | 'Possible rename'
  details?: string[]
}

const CHANGE_LABELS: [keyof ExtensionField, string][] = [
  ['type', 'Type'], ['unit', 'Unit'], ['builtin', 'Filled by'], ['description', 'Notes'],
  ['enum_values', 'Allowed values'], ['required', 'Required'], ['default', 'Default'],
]

export function compareFields(previous: SchemaFieldRow[], next: SchemaFieldRow[]): SchemaChange[] {
  const previousByKey = new Map(previous.filter((row) => row.field).map((row) => [row.key, row]))
  const nextByKey = new Map(next.filter((row) => row.field).map((row) => [row.key, row]))
  const removed = [...previousByKey.values()].filter((row) => !nextByKey.has(row.key))
  const added = [...nextByKey.values()].filter((row) => !previousByKey.has(row.key))
  const signature = ({ field }: SchemaFieldRow) => stableJson({ ...field, key: undefined })
  const renames = new Set<SchemaFieldRow>()
  const changes: SchemaChange[] = removed.map((before) => {
    const candidates = added.filter((row) => signature(row) === signature(before))
    if (candidates.length === 1 && removed.filter((row) => signature(row) === signature(before)).length === 1) {
      renames.add(candidates[0])
      return { before, after: candidates[0], kind: 'Possible rename' }
    }
    return { before, kind: 'Removed' }
  })
  for (const after of added) {
    if (!renames.has(after)) changes.push({ after, kind: 'Added' })
  }
  for (const [key, after] of nextByKey) {
    const before = previousByKey.get(key)
    if (!before) continue
    const details = CHANGE_LABELS.flatMap(([property, label]) => {
      const was = before.field?.[property]
      const now = after.field?.[property]
      return stableJson(was) === stableJson(now) ? [] : [`${label}: ${formatValue(was)} → ${formatValue(now)}`]
    })
    if (details.length) changes.push({ before, after, kind: 'Changed', details })
  }
  return changes
}

function formatValue(value: unknown) {
  return value === undefined ? '—' : typeof value === 'string' ? value : JSON.stringify(value)
}
