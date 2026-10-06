/**
 * Keep an entity's four symbol fields in step: domain, affiliation, CoT type and SIDC (stored as
 * MIL-STD-2525D). Whichever one changes, the others follow:
 * - **affiliation:** the SIDC's standard identity and the CoT type's affiliation atom;
 * - **domain:** a symbol of another domain becomes that domain's generic symbol (its symbol set,
 *   unspecified entity; the CoT type `a-<affiliation>-<dimension>`);
 * - **SIDC:** the CoT type (2525C crosswalk), domain and affiliation; a 2525C code is stored as
 *   its 2525D equivalent;
 * - **CoT type:** the SIDC (crosswalk, else the domain's generic symbol), domain and affiliation.
 * Identity digits, symbol sets and dimensions match the server's (crates/ot-core/src/sidc.rs).
 * A field mid-edit that is not a valid code yet changes nothing.
 */
import crosswalk from './crosswalk.json'

export interface SymbolFields {
  domain: string
  affiliation: string
  cot_type: string
  sidc: string
}

export type SymbolField = keyof SymbolFields

const TABLE = crosswalk as Record<string, string>

/** 2525C dimension + function ("G-UCI") -> 2525D symbol set + entity code; the first wins. */
const REVERSE: Record<string, string> = {}
for (const [d, c] of Object.entries(TABLE)) REVERSE[c] ??= d

const DOMAIN_DIM: Record<string, string> = { air: 'A', surface: 'S', subsurface: 'U', ground: 'G', space: 'P' }
const DIM_DOMAIN: Record<string, string> = Object.fromEntries(Object.entries(DOMAIN_DIM).map(([k, v]) => [v, k]))
/** The symbol set of a domain's generic symbol. */
const DOMAIN_SET: Record<string, string> = { air: '01', surface: '30', subsurface: '35', ground: '10', space: '05' }
/** The domain a 2525D symbol set draws in (as the server reads it). */
const SET_DOMAIN: Record<string, string> = {
  '01': 'air', '02': 'air', '51': 'air',
  '05': 'space', '06': 'space', '50': 'space',
  '10': 'ground', '11': 'ground', '15': 'ground', '20': 'ground', '27': 'ground', '52': 'ground',
  '30': 'surface', '53': 'surface',
  '35': 'subsurface', '36': 'subsurface', '54': 'subsurface',
}

const ATOM: Record<string, string> = {
  pending: 'p', unknown: 'u', assumed_friend: 'a', friend: 'f', neutral: 'n',
  suspect: 's', hostile: 'h', joker: 'j', faker: 'k', none: 'o',
}
const ATOM_AFF: Record<string, string> = Object.fromEntries(Object.entries(ATOM).map(([k, v]) => [v, k]))
/** 2525D context (reality 0, exercise 1) and standard identity digits per affiliation. */
const D_IDENTITY: Record<string, [string, string]> = {
  pending: ['0', '0'], unknown: ['0', '1'], assumed_friend: ['0', '2'], friend: ['0', '3'], neutral: ['0', '4'],
  suspect: ['0', '5'], hostile: ['0', '6'], joker: ['1', '5'], faker: ['1', '6'],
}

const isD = (s: string) => /^\d{20}$/.test(s)
const isC = (s: string) => /^[SGWIOE][A-Z\-*][A-Z\-*][A-Z\-*][A-Z0-9\-*]{6,11}$/i.test(s)
const cotAtoms = (t: string) => (/^a-[a-z]-[A-Z]/.test(t) ? t.split('-') : null)

function dAffiliation(sidc: string): string | undefined {
  const exercise = sidc[2] === '1'
  const digit = sidc[3]
  if (digit === '5') return exercise ? 'joker' : 'suspect'
  if (digit === '6') return exercise ? 'faker' : 'hostile'
  return { '0': 'pending', '1': 'unknown', '2': 'assumed_friend', '3': 'friend', '4': 'neutral' }[digit]
}

/** A 2525D SIDC with another identity (unchanged for an affiliation 2525D has none for). */
function withIdentity(sidc: string, affiliation: string): string {
  const id = D_IDENTITY[affiliation]
  return id ? sidc.slice(0, 2) + id[0] + id[1] + sidc.slice(4) : sidc
}

/** The domain's generic 2525D symbol, with this affiliation. */
function genericSidc(domain: string, affiliation: string): string | null {
  const set = DOMAIN_SET[domain]
  return set ? withIdentity(`1000${set}${'0'.repeat(14)}`, affiliation || 'unknown') : null
}

/** The CoT type for a 2525D SIDC: the crosswalk's dimension and function, else the set's dimension. */
export function cotForSidc(sidc: string): string | null {
  if (!isD(sidc)) return null
  const atom = ATOM[dAffiliation(sidc) ?? 'unknown'] ?? 'u'
  const legacy = TABLE[sidc.slice(4, 6) + sidc.slice(10, 16)]
  const [dim, func = ''] = legacy ? legacy.split('-') : [DOMAIN_DIM[SET_DOMAIN[sidc.slice(4, 6)] ?? ''] ?? 'X']
  return ['a', atom, dim, ...func.split('')].join('-')
}

/** The 2525D SIDC for a CoT type: the crosswalk (its nearest listed parent), else the domain's generic symbol. */
export function sidcForCot(cot: string): string | null {
  const parts = cotAtoms(cot)
  if (!parts) return null
  const affiliation = ATOM_AFF[parts[1]] ?? 'unknown'
  const dim = parts[2]
  let func = parts.slice(3).join('')
  while (func) {
    const d = REVERSE[`${dim}-${func}`]
    if (d) return withIdentity(`1000${d.slice(0, 2)}0000${d.slice(2)}0000`, affiliation)
    func = func.slice(0, -1)
  }
  return genericSidc(DIM_DOMAIN[dim] ?? '', affiliation)
}

/** A 2525C (letter) SIDC as the CoT type it spells out. */
function cotForC(c: string): string | null {
  const s = c.toUpperCase().replace(/\*/g, '-')
  if (s[0] !== 'S') return null
  const atom = s[1] === 'G' ? 'p' : s[1] === 'W' || s[1] === '-' ? 'u' : s[1].toLowerCase()
  const func = s.slice(4, 10).replace(/-+$/, '')
  return ['a', atom, s[2], ...func.split('')].join('-')
}

function domainOfSidc(sidc: string): string | undefined {
  return SET_DOMAIN[sidc.slice(4, 6)]
}

/** The fields after `changed` was set to its value in `f`, the others following it. */
export function syncSymbol(f: SymbolFields, changed: SymbolField): SymbolFields {
  const out = { ...f }
  const sidc = f.sidc.trim()
  const cot = f.cot_type.trim()
  switch (changed) {
    case 'affiliation': {
      if (!f.affiliation) return out
      if (isD(sidc)) out.sidc = withIdentity(sidc, f.affiliation)
      const parts = cotAtoms(cot)
      if (parts && ATOM[f.affiliation]) out.cot_type = [parts[0], ATOM[f.affiliation], ...parts.slice(2)].join('-')
      if (!sidc && !cot && f.domain) {
        // No symbol yet: the domain's generic one, with this affiliation.
        out.sidc = genericSidc(f.domain, f.affiliation) ?? ''
        out.cot_type = cotForSidc(out.sidc) ?? ''
      }
      return out
    }
    case 'domain': {
      if (!f.domain) return out
      const current = isD(sidc) ? domainOfSidc(sidc) : cotAtoms(cot) ? DIM_DOMAIN[cotAtoms(cot)![2]] : undefined
      if (current === f.domain) return out
      out.sidc = genericSidc(f.domain, f.affiliation || (isD(sidc) ? (dAffiliation(sidc) ?? '') : '')) ?? out.sidc
      out.cot_type = cotForSidc(out.sidc) ?? out.cot_type
      return out
    }
    case 'sidc': {
      let d = sidc
      if (!isD(d) && isC(d)) {
        const viaCot = cotForC(d)
        d = (viaCot && sidcForCot(viaCot)) ?? d
      }
      if (!isD(d)) return out
      out.sidc = d
      out.cot_type = cotForSidc(d) ?? out.cot_type
      out.domain = domainOfSidc(d) ?? out.domain
      out.affiliation = dAffiliation(d) ?? out.affiliation
      return out
    }
    case 'cot_type': {
      const parts = cotAtoms(cot)
      if (!parts) return out
      out.sidc = sidcForCot(cot) ?? out.sidc
      out.domain = DIM_DOMAIN[parts[2]] ?? out.domain
      out.affiliation = ATOM_AFF[parts[1]] ?? out.affiliation
      return out
    }
  }
}
