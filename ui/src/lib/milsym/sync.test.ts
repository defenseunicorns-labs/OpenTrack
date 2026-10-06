import { describe, expect, it } from 'vitest'
import { cotForSidc, sidcForCot, syncSymbol, type SymbolFields } from './sync'

const INF_F = '10031000001211000000' // friend infantry
const blank: SymbolFields = { domain: '', affiliation: '', cot_type: '', sidc: '' }

describe('symbol field sync', () => {
  it('maps between 2525D and CoT both ways', () => {
    expect(cotForSidc(INF_F)).toBe('a-f-G-U-C-I')
    expect(sidcForCot('a-f-G-U-C-I')).toBe(INF_F)
    // A CoT function the crosswalk lacks falls back to its nearest listed parent.
    expect(sidcForCot('a-h-G-U-C-I-9')).toBe('10061000001211000000')
    expect(sidcForCot('a-f-G-U-C-I-Z')).toBe('10031000001211020000') // motorized infantry
    // Joker and faker are exercise identities.
    expect(cotForSidc('10151000001211000000')).toBe('a-j-G-U-C-I')
    expect(sidcForCot('a-k-A')).toBe('10160100000000000000')
  })

  it('an affiliation change rewrites both codes', () => {
    const f = { domain: 'ground', affiliation: 'hostile', cot_type: 'a-f-G-U-C-I', sidc: INF_F }
    expect(syncSymbol(f, 'affiliation')).toEqual({ ...f, cot_type: 'a-h-G-U-C-I', sidc: '10061000001211000000' })
    expect(syncSymbol({ ...f, affiliation: 'joker' }, 'affiliation').sidc).toBe('10151000001211000000')
  })

  it('a domain change gives the new domain generic symbol, and leaves a matching one', () => {
    const f = { domain: 'surface', affiliation: 'friend', cot_type: 'a-f-G-U-C-I', sidc: INF_F }
    expect(syncSymbol(f, 'domain')).toEqual({ ...f, cot_type: 'a-f-S', sidc: '10033000000000000000' })
    expect(syncSymbol({ ...f, domain: 'ground' }, 'domain')).toEqual({ ...f, domain: 'ground' })
  })

  it('a symbol code sets the other code, domain and affiliation', () => {
    expect(syncSymbol({ ...blank, sidc: INF_F }, 'sidc')).toEqual({ domain: 'ground', affiliation: 'friend', cot_type: 'a-f-G-U-C-I', sidc: INF_F })
    expect(syncSymbol({ ...blank, cot_type: 'a-n-S' }, 'cot_type')).toEqual({ domain: 'surface', affiliation: 'neutral', cot_type: 'a-n-S', sidc: '10043000000000000000' })
    // A 2525C code is stored as its 2525D equivalent.
    expect(syncSymbol({ ...blank, sidc: 'SFGPUCI----' }, 'sidc').sidc).toBe(INF_F)
  })

  it('a code mid-edit, or a blank choice, changes nothing', () => {
    const f = { domain: 'ground', affiliation: 'friend', cot_type: 'a-f-G-U-C-I', sidc: '1003100000' }
    expect(syncSymbol(f, 'sidc')).toEqual(f)
    expect(syncSymbol({ ...f, cot_type: 'a-' }, 'cot_type')).toEqual({ ...f, cot_type: 'a-' })
    expect(syncSymbol({ ...f, domain: '' }, 'domain')).toEqual({ ...f, domain: '' })
  })

  it('an affiliation with no symbol yet makes the domain generic one', () => {
    expect(syncSymbol({ ...blank, domain: 'air', affiliation: 'hostile' }, 'affiliation')).toEqual({
      domain: 'air',
      affiliation: 'hostile',
      cot_type: 'a-h-A',
      sidc: '10060100000000000000',
    })
  })
})
