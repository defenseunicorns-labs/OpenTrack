import ms from 'milsymbol'
import type { TrackMessage } from '../api/client'
import { cotToSidc } from './cotToSidc'

type Sidc = TrackMessage['sidc']

/**
 * The code milsymbol draws for a published SIDC. milsymbol reads 2525C and 2525D codes as given
 * but does not understand CoT types (`a-n-S` is invalid to it), so CoT goes through OpenStare's
 * converter first, as OpenStare's map does.
 */
export function drawableSidc(sidc: Sidc): string | null {
  if (sidc.standard === 'cot') return cotToSidc(sidc.code)
  return sidc.code || null
}

const cache = new Map<string, string | null>()

/**
 * The symbol as an SVG data URL, for an `<img>` (so the SVG never runs as markup in the page).
 * Colours are milsymbol's standard affiliation fills, as on OpenStare's map. Cached per code and
 * size; null when the code cannot be drawn.
 */
export function symbolUrl(sidc: Sidc, size = 28): string | null {
  const code = drawableSidc(sidc)
  if (!code) return null
  const key = `${code}|${size}`
  if (!cache.has(key)) {
    try {
      const sym = new ms.Symbol(code, { size, square: true })
      cache.set(key, sym.isValid() ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(sym.asSVG())}` : null)
    } catch {
      cache.set(key, null)
    }
  }
  return cache.get(key) ?? null
}

/** Human name of a SIDC's standard. */
export const standardName = (s: Sidc['standard']) => (s === 'cot' ? 'CoT type' : `MIL-STD-${s.toUpperCase()}`)
