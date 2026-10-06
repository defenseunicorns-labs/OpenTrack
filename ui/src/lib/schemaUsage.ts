/** How sources and the output schema meet: who feeds each field, and what no field publishes. */
import type { ExtensionField, SchemaOverview, SourceRow } from '../api/client'
import { fieldMap } from './pipeline'

/** Ids of the sources whose pipeline publishes into `attributes.<key>`. */
export function usedBy(key: string, sources: SourceRow[], schema: SchemaOverview): string[] {
  return sources
    .filter((s) => fieldMap(s.spec, schema).some((r) => r.published.some((d) => d.kind === 'attribute' && d.text === `attributes.${key}`)))
    .map((s) => s.id)
}

export interface Unpublished {
  /** The OpenTrack field a pipeline sets (`ext.x` or a core field such as `platform.type_code`). */
  target: string
  /** `source: feed field` pairs that set it. */
  from: string[]
  /** A schema field that would publish it. */
  suggestion: ExtensionField
}

const KEY_RE = /^[a-z][a-z0-9_]{0,63}$/

/** Whether `key` is a valid output field name (the server's rule: lowercase, digits, `_`). */
export function validKey(key: string, reserved: string[] = []): string | null {
  if (!KEY_RE.test(key)) return 'lowercase letters, digits and _ only, starting with a letter'
  if (reserved.includes(key)) return 'reserved'
  return null
}

/** Values enabled sources set that no schema field publishes, each with a field that would. */
export function unpublished(sources: SourceRow[], schema: SchemaOverview): Unpublished[] {
  const by = new Map<string, Unpublished>()
  for (const s of sources.filter((s) => s.enabled)) {
    for (const r of fieldMap(s.spec, schema)) {
      if (!r.published.some((d) => d.kind === 'none')) continue
      const entry = by.get(r.target) ?? { target: r.target, from: [], suggestion: suggest(r.target, schema) }
      const where = `${s.id}: ${r.from}`
      if (!entry.from.includes(where)) entry.from.push(where)
      by.set(r.target, entry)
    }
  }
  return [...by.values()].sort((a, b) => a.target.localeCompare(b.target))
}

function suggest(target: string, schema: SchemaOverview): ExtensionField {
  if (target.startsWith('ext.')) {
    return { key: target.slice(4), type: 'string', description: `From feed mappings (${target}).` }
  }
  // A core field: link the built-in that reads it, preferring one whose main input it is.
  const readers = (schema.builtins ?? []).filter((b) => (b.reads ?? []).includes(target))
  const b = readers.find((x) => x.reads?.[0] === target) ?? readers[0]
  const key = target.split('.').pop()!.replace(/[^a-z0-9_]/g, '_')
  return { key, type: b?.type ?? 'string', builtin: b?.name, description: `OpenTrack's ${target}.` }
}
