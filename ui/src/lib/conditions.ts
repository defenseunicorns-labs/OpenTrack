/**
 * The filter builder's model of a pipeline condition (crates/ot-source/src/expr.rs `Condition`):
 * rows of "field, test, value", matched all or any. Conditions the rows cannot show (nested groups,
 * value specs, a `not` around anything but a text test) stay in JSON.
 */

type Obj = Record<string, unknown>
const isObj = (v: unknown): v is Obj => typeof v === 'object' && v !== null && !Array.isArray(v)

export type Op =
  | 'eq'
  | 'ne'
  | 'in'
  | 'not_in'
  | 'starts_with'
  | 'not_starts_with'
  | 'contains'
  | 'not_contains'
  | 'matches'
  | 'not_matches'
  | 'gt'
  | 'gte'
  | 'lt'
  | 'lte'
  | 'present'
  | 'missing'

/** What a test takes: one value, a list, a number, text, or nothing. */
type Takes = 'one' | 'list' | 'number' | 'text' | 'none'

export const OPS: { value: Op; label: string; takes: Takes }[] = [
  { value: 'eq', label: 'is', takes: 'one' },
  { value: 'ne', label: 'is not', takes: 'one' },
  { value: 'in', label: 'is one of', takes: 'list' },
  { value: 'not_in', label: 'is none of', takes: 'list' },
  { value: 'starts_with', label: 'starts with', takes: 'text' },
  { value: 'not_starts_with', label: 'does not start with', takes: 'text' },
  { value: 'contains', label: 'contains', takes: 'text' },
  { value: 'not_contains', label: 'does not contain', takes: 'text' },
  { value: 'matches', label: 'matches pattern', takes: 'text' },
  { value: 'not_matches', label: 'does not match pattern', takes: 'text' },
  { value: 'gt', label: 'is more than', takes: 'number' },
  { value: 'gte', label: 'is at least', takes: 'number' },
  { value: 'lt', label: 'is less than', takes: 'number' },
  { value: 'lte', label: 'is at most', takes: 'number' },
  { value: 'present', label: 'is present', takes: 'none' },
  { value: 'missing', label: 'is missing', takes: 'none' },
]

export const takes = (op: Op): Takes => OPS.find((o) => o.value === op)?.takes ?? 'one'

/** The tests a `not` may wrap in the builder: the text tests, shown as "does not …". */
const NEGATED: Partial<Record<string, Op>> = { starts_with: 'not_starts_with', contains: 'not_contains', matches: 'not_matches' }

/** One row: a field (path), a test, and the value as typed. */
export interface Rule {
  path: string
  op: Op
  value: string
}

export interface Built {
  match: 'all' | 'any'
  rules: Rule[]
}

const typed = (v: unknown) => (typeof v === 'string' ? v : JSON.stringify(v))

/** A single test (or a `not` around a text test) as a row, or null when a row cannot show it. */
function toRule(c: unknown): Rule | null {
  if (!isObj(c)) return null
  if (c.not !== undefined) {
    const inner = toRule(c.not)
    const negated = inner && NEGATED[inner.op]
    return inner && negated ? { ...inner, op: negated } : null
  }
  if (typeof c.path !== 'string') return null
  const keys = Object.keys(c).filter((k) => k !== 'path')
  if (keys.length !== 1) return null
  const [k] = keys
  const v = c[k]
  if (k === 'exists') return { path: c.path, op: v ? 'present' : 'missing', value: '' }
  if (!OPS.some((o) => o.value === k)) return null
  if (k === 'in' || k === 'not_in') return Array.isArray(v) ? { path: c.path, op: k, value: v.map(typed).join(', ') } : null
  return { path: c.path, op: k as Op, value: typed(v) }
}

/** The rows a condition shows as, or null when it needs the JSON editor. Nothing yet: no rows. */
export function toBuilt(c: unknown): Built | null {
  if (c === undefined || c === null) return { match: 'all', rules: [] }
  if (isObj(c)) {
    for (const match of ['all', 'any'] as const) {
      const list = c[match]
      if (Array.isArray(list) && Object.keys(c).length === 1) {
        const rules = list.map(toRule)
        return rules.every((r): r is Rule => r !== null) ? { match, rules } : null
      }
    }
  }
  const rule = toRule(c)
  return rule ? { match: 'all', rules: [rule] } : null
}

/** A typed value as the field's samples hold it: a number or boolean when the samples do. */
function asValue(text: string, kind: string | undefined): unknown {
  const t = text.trim()
  if (kind === 'number' && t !== '' && !Number.isNaN(Number(t))) return Number(t)
  if (kind === 'boolean' && (t === 'true' || t === 'false')) return t === 'true'
  return t
}

/** The test a row makes, or undefined while it is incomplete (no field, or no value it needs). */
export function fromRule(r: Rule, kinds: Record<string, string> = {}): Obj | undefined {
  const path = r.path.trim()
  if (!path) return undefined
  const kind = kinds[path]
  switch (takes(r.op)) {
    case 'none':
      return { path, exists: r.op === 'present' }
    case 'list': {
      const list = r.value
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean)
      return list.length ? { path, [r.op]: list.map((s) => asValue(s, kind)) } : undefined
    }
    case 'number': {
      const n = r.value.trim() === '' ? NaN : Number(r.value)
      return Number.isNaN(n) ? undefined : { path, [r.op]: n }
    }
    case 'text': {
      if (r.value === '') return undefined
      const negated = r.op.startsWith('not_')
      const test = { path, [negated ? r.op.slice(4) : r.op]: r.value }
      return negated ? { not: test } : test
    }
    default:
      return r.value.trim() === '' ? undefined : { path, [r.op]: asValue(r.value, kind) }
  }
}

/** The condition the rows make: one test alone, else all/any of them; undefined with none complete. */
export function fromBuilt(b: Built, kinds: Record<string, string> = {}): Obj | undefined {
  const tests = b.rules.map((r) => fromRule(r, kinds)).filter((t): t is Obj => t !== undefined)
  if (tests.length === 0) return undefined
  if (tests.length === 1) return tests[0]
  return { [b.match]: tests }
}

/** A field the samples hold: its path, the kind of value, and one example. */
export interface SampleField {
  path: string
  kind: string
  example: unknown
}

/** Every leaf field in `objects` (objects nested by dots; arrays are leaves), first example kept. */
export function sampleFields(objects: unknown[]): SampleField[] {
  const seen = new Map<string, SampleField>()
  const walk = (v: unknown, path: string, depth: number) => {
    if (isObj(v) && depth < 8) {
      for (const [k, child] of Object.entries(v)) walk(child, path ? `${path}.${k}` : k, depth + 1)
      return
    }
    if (!path) return
    const known = seen.get(path)
    if (known && known.example !== null) return
    seen.set(path, { path, kind: v === null ? (known?.kind ?? 'null') : Array.isArray(v) ? 'array' : typeof v, example: v })
  }
  for (const o of objects) walk(o, '', 0)
  return [...seen.values()].sort((a, b) => a.path.localeCompare(b.path))
}
