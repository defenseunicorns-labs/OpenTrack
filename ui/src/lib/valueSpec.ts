/**
 * The form view of a mapping value spec (see ot-source `expr.rs`). Specs that are a path, the
 * first of several paths, a fixed value or a template, optionally with transforms, ignored
 * values, a lookup table and a default, are edited as fields; anything richer (cases,
 * arithmetic, ranges, keyed lookups) stays JSON.
 */
import type { ValueSpec } from './pipeline'

export type Mode = 'path' | 'first' | 'const' | 'template' | 'advanced'

export interface ValueForm {
  mode: Mode
  path: string
  first: string
  constant: string
  template: string
  transforms: string
  nullIf: string
  table: Record<string, unknown> | null
  tableDefault: string
  defaultValue: string
}

const SIMPLE = new Set(['path', 'first', 'const', 'template', 'transforms', 'null_if', 'table', 'table_default', 'default'])

/** A literal as the user types it: strings bare, anything else as JSON. */
export const showLiteral = (v: unknown) => (v === undefined || v === null ? '' : typeof v === 'string' ? v : JSON.stringify(v))
/** The user's text back to a literal: JSON when it parses (numbers, true, null...), else the string. */
export function readLiteral(t: string): unknown {
  const s = t.trim()
  if (s === '') return undefined
  try {
    return JSON.parse(s)
  } catch {
    return t
  }
}

const list = (s: string) =>
  s
    .split(',')
    .map((x) => x.trim())
    .filter(Boolean)

const empty = (mode: Mode): ValueForm => ({
  mode,
  path: '',
  first: '',
  constant: '',
  template: '',
  transforms: '',
  nullIf: '',
  table: null,
  tableDefault: '',
  defaultValue: '',
})

/** The form for a spec, or mode `advanced` when it needs JSON. */
export function toForm(spec: ValueSpec | undefined): ValueForm {
  if (spec === undefined) return empty('path')
  if (typeof spec === 'string') return { ...empty('path'), path: spec }
  if (!Object.keys(spec).every((k) => SIMPLE.has(k))) return empty('advanced')
  const sources = ['path', 'first', 'const', 'template'].filter((k) => spec[k] !== undefined)
  if (sources.length !== 1) return empty('advanced')
  if (spec.first !== undefined && !(Array.isArray(spec.first) && spec.first.every((x) => typeof x === 'string'))) return empty('advanced')
  // Text that reads as JSON ("0", "true") would come back as a number or boolean: keep it in JSON.
  const literals = [spec.const, spec.default, spec.table_default, ...(Array.isArray(spec.null_if) ? spec.null_if : [])]
  if (literals.some((v) => typeof v === 'string' && readLiteral(v) !== v)) return empty('advanced')
  if (Array.isArray(spec.null_if) && spec.null_if.some((v) => typeof v === 'string' && v.includes(','))) return empty('advanced')
  // Transforms are written with commas inside some arguments (`replace:,:`), which a comma list cannot hold.
  // Nor can it keep a transform's leading or trailing spaces (`replace:@: ` replaces @ with a space).
  if (Array.isArray(spec.transforms) && spec.transforms.some((t) => String(t).includes(',') || String(t) !== String(t).trim())) {
    return empty('advanced')
  }
  const mode = (sources[0] === 'const' ? 'const' : sources[0]) as Mode
  return {
    mode,
    path: typeof spec.path === 'string' ? spec.path : '',
    first: Array.isArray(spec.first) ? spec.first.join(', ') : '',
    constant: showLiteral(spec.const),
    template: typeof spec.template === 'string' ? spec.template : '',
    transforms: Array.isArray(spec.transforms) ? spec.transforms.join(', ') : '',
    nullIf: Array.isArray(spec.null_if) ? spec.null_if.map(showLiteral).join(', ') : '',
    table: spec.table && typeof spec.table === 'object' ? (spec.table as Record<string, unknown>) : null,
    tableDefault: showLiteral(spec.table_default),
    defaultValue: showLiteral(spec.default),
  }
}

/** The spec for a form (not for `advanced`, whose JSON is the spec). A bare path stays a string. */
export function fromForm(f: ValueForm): ValueSpec {
  const out: Record<string, unknown> = {}
  if (f.mode === 'path') out.path = f.path.trim()
  if (f.mode === 'first') out.first = list(f.first)
  if (f.mode === 'const') out.const = readLiteral(f.constant) ?? ''
  if (f.mode === 'template') out.template = f.template
  const nulls = list(f.nullIf).map(readLiteral)
  if (nulls.length) out.null_if = nulls
  const transforms = list(f.transforms)
  if (transforms.length) out.transforms = transforms
  if (f.table) {
    out.table = f.table
    const d = readLiteral(f.tableDefault)
    if (d !== undefined) out.table_default = d
  }
  const d = readLiteral(f.defaultValue)
  if (d !== undefined) out.default = d
  if (f.mode === 'path' && Object.keys(out).length === 1) return out.path as string
  return out
}

/** Transforms the server knows; those ending in `:` take an argument. */
export const TRANSFORMS = [
  'trim',
  'upper',
  'lower',
  'nonempty',
  'string',
  'number',
  'bool',
  'time',
  'time_unix_s',
  'time_unix_ms',
  'knots_to_mps',
  'feet_to_m',
  'fpm_to_mps',
  'kmh_to_mps',
  'nm_to_m',
  'wrap360',
  'hex',
  'scale:',
  'offset:',
  'round:',
  'replace:',
  'split:',
  'regex:',
  'bits_any:',
]

/** Inputs as tall as the selects beside them (28px), for dense forms. */
export const INPUT = { height: 28, fontSize: 12 }
