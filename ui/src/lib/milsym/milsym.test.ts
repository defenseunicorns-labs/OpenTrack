import { describe, expect, it } from 'vitest'
import { buildSidc, parseSidc } from './sidc'
import { entities, selectedCode, selectionFor, subtypes, symbolSets, types } from './catalog'
import { cotForSidc as sidcToCot } from './sync'

describe('symbol designer model', () => {
  it('builds and reads 2525D SIDCs', () => {
    const sidc = buildSidc('10', '121100', 'friend')
    expect(sidc).toBe('10031000001211000000')
    expect(parseSidc(sidc)).toEqual({ symbolSet: '10', code: '121100', affiliation: 'friend' })
    expect(parseSidc('SFGPUCI---------')).toBeNull()
  })

  it('writes the CoT type the 2525C crosswalk gives, else affiliation and dimension', () => {
    expect(sidcToCot(buildSidc('10', '121100', 'friend'))).toBe('a-f-G-U-C-I')
    expect(sidcToCot(buildSidc('01', '110100', 'hostile'))).toBe('a-h-A-M-F')
    expect(sidcToCot(buildSidc('30', '120100', 'neutral'))).toBe('a-n-S-C-L-C-V')
    expect(sidcToCot(buildSidc('35', '999999', 'unknown'))).toBe('a-u-U')
    expect(sidcToCot('not a sidc')).toBeNull()
  })

  it('offers point symbols only, and reopens a design from its code', () => {
    const sets = symbolSets().map((s) => s.code)
    expect(sets).toContain('10')
    expect(sets).toContain('25')
    // Control measures: no line or area graphics anywhere in the cascade.
    for (const e of entities('25'))
      for (const t of types('25', e)) for (const s of subtypes('25', e, t)) expect(s.label).not.toMatch(/^\{.*reserved/i)
    const labels = entities('25').flatMap((e) => types('25', e).map((t) => t.label))
    expect(labels).not.toContain('Phase Line') // a line
    expect(labels).not.toContain('Bypass') // drawn from three control points
    expect(labels).toContain('Destroy') // a point
    const sel = selectionFor('10', '121100')
    expect(sel.entity?.label).toBe('Movement and Maneuver')
    expect(selectedCode(sel)).toBe('121100')
  })
})
