/**
 * MIL-STD-2525D 20-digit numeric SIDCs: build one from the designer's choices, and read one back.
 * Started from OpenStare's sidcBuilder.ts (analysis/tactical); OpenTrack's own copy from here on.
 *
 * Layout (0-indexed): 0-1 version "10" (2525D) · 2 context "0" (reality) · 3 standard identity
 * (0/1 unknown, 2/3 friend, 4 neutral, 5/6 hostile) · 4-5 symbol set · 6 status · 7 HQ/TF/dummy
 * · 8-9 amplifier/echelon · 10-15 entity code (entity, type, subtype) · 16-17 modifier 1 ·
 * 18-19 modifier 2.
 */

export type Affiliation = 'unknown' | 'friend' | 'neutral' | 'hostile'

const AFFIL_DIGIT: Record<Affiliation, string> = { unknown: '1', friend: '3', neutral: '4', hostile: '6' }
const DIGIT_AFFIL: Record<string, Affiliation> = {
  '0': 'unknown',
  '1': 'unknown',
  '2': 'friend',
  '3': 'friend',
  '4': 'neutral',
  '5': 'hostile',
  '6': 'hostile',
}

/** A 20-digit 2525D SIDC for a symbol set, an entity code and an affiliation (no modifiers). */
export function buildSidc(symbolSet: string, code: string, affiliation: Affiliation): string {
  return `100${AFFIL_DIGIT[affiliation]}${symbolSet}0000${code}0000`
}

export interface ParsedSidc {
  symbolSet: string
  code: string
  affiliation: Affiliation
}

/** A 2525D SIDC's symbol set, entity code and affiliation; null when it is not one. */
export function parseSidc(sidc: string): ParsedSidc | null {
  const s = sidc.trim()
  if (!/^\d{20}$/.test(s)) return null
  return { symbolSet: s.slice(4, 6), code: s.slice(10, 16), affiliation: DIGIT_AFFIL[s[3]] ?? 'unknown' }
}
