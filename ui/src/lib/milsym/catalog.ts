/**
 * The 2525D symbol catalog (the `mil-std-2525` package's tables) as the designer's cascading
 * lists: symbol set, then entity, type and subtype. Point symbols only: control measures drawn as
 * lines or areas (symbol set 25) and the standard's reserved placeholders are left out.
 * Started from OpenStare's tacticalCatalog.ts and builderSelection.ts; OpenTrack's own copy.
 */
import { ms2525d, type Ms2525Row } from 'mil-std-2525'

export interface CatalogOption {
  code: string
  label: string
}

/** The cascade's selection; the deepest chosen level's code is the symbol's entity code. */
export interface Selection {
  entity: CatalogOption | null
  type: CatalogOption | null
  subtype: CatalogOption | null
}

export const NO_SELECTION: Selection = { entity: null, type: null, subtype: null }

type Row = Ms2525Row

const rows = (symbolSet: string): Row[] => ms2525d[symbolSet]?.mainIcon ?? []
const RESERVED = /^\{.*reserved.*\}$/i
const byLabel = (a: CatalogOption, b: CatalogOption) => a.label.localeCompare(b.label)

/**
 * A point symbol: anything outside control measures; in control measures only rows the standard
 * renders as a single point ("Point"). Its untagged rows are category headings (Fires Areas…);
 * "Point12" and the like are drawn from several control points (Bypass, Canalize), and line and
 * area graphics are left out.
 */
function isPoint(symbolSet: string, code: string): boolean {
  if (symbolSet !== '25') return true
  return rows(symbolSet).find((x) => x.Code === code)?.['Geometric Rendering'] === 'Point'
}

function real(symbolSet: string, o: CatalogOption): boolean {
  return !RESERVED.test(o.label.trim()) && isPoint(symbolSet, o.code)
}

export function symbolSets(): CatalogOption[] {
  return Object.keys(ms2525d)
    .map((code) => ({ code, label: String(ms2525d[code].name) }))
    .filter((s) => entities(s.code).length > 0)
    .sort((a, b) => a.code.localeCompare(b.code))
}

function allEntities(symbolSet: string): CatalogOption[] {
  const seen = new Map<string, string>()
  for (const r of rows(symbolSet)) {
    if (!r.Entity) continue
    if (!seen.has(r.Entity) || (!r['Entity Type'] && !r['Entity Subtype'])) seen.set(r.Entity, r.Code)
  }
  return [...seen].map(([label, code]) => ({ code, label }))
}

function allTypes(symbolSet: string, entity: string): CatalogOption[] {
  const seen = new Map<string, string>()
  for (const r of rows(symbolSet)) {
    if (r.Entity !== entity || !r['Entity Type']) continue
    if (!seen.has(r['Entity Type']) || !r['Entity Subtype']) seen.set(r['Entity Type'], r.Code)
  }
  return [...seen].map(([label, code]) => ({ code, label }))
}

function allSubtypes(symbolSet: string, entity: string, type: string): CatalogOption[] {
  return rows(symbolSet)
    .filter((r) => r.Entity === entity && r['Entity Type'] === type && r['Entity Subtype'])
    .map((r) => ({ code: r.Code, label: r['Entity Subtype'] }))
}

/** Subtypes that are point symbols. */
export function subtypes(symbolSet: string, entity: CatalogOption, type: CatalogOption): CatalogOption[] {
  return allSubtypes(symbolSet, entity.label, type.label).filter((s) => real(symbolSet, s)).sort(byLabel)
}

/** Types that are a point symbol themselves or have one among their subtypes. */
export function types(symbolSet: string, entity: CatalogOption): CatalogOption[] {
  return allTypes(symbolSet, entity.label)
    .filter((t) => !RESERVED.test(t.label.trim()))
    .filter((t) => {
      const subs = allSubtypes(symbolSet, entity.label, t.label)
      return subs.length === 0 ? isPoint(symbolSet, t.code) : subs.some((s) => real(symbolSet, s))
    })
    .sort(byLabel)
}

/** Entities that are a point symbol themselves or have one beneath them. */
export function entities(symbolSet: string): CatalogOption[] {
  return allEntities(symbolSet)
    .filter((e) => !RESERVED.test(e.label.trim()))
    .filter((e) => (allTypes(symbolSet, e.label).length === 0 ? isPoint(symbolSet, e.code) : types(symbolSet, e).length > 0))
    .sort(byLabel)
}

/** The entity code of the deepest chosen level ("000000" with nothing chosen). */
export function selectedCode(s: Selection): string {
  return s.subtype?.code ?? s.type?.code ?? s.entity?.code ?? '000000'
}

/** The selection an entity code was made from (to reopen a symbol in the designer). */
export function selectionFor(symbolSet: string, code: string): Selection {
  for (const entity of entities(symbolSet)) {
    if (entity.code === code) return { entity, type: null, subtype: null }
    for (const type of types(symbolSet, entity)) {
      if (type.code === code) return { entity, type, subtype: null }
      const subtype = subtypes(symbolSet, entity, type).find((s) => s.code === code)
      if (subtype) return { entity, type, subtype }
    }
  }
  return NO_SELECTION
}
