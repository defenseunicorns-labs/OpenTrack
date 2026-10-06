import { describe, expect, it } from 'vitest'
import { fromBuilt, fromRule, sampleFields, toBuilt } from './conditions'

describe('conditions', () => {
  it('round-trips the conditions the builder can show', () => {
    const conditions = [
      { path: 'source_track_key', starts_with: 'tms-' },
      { not: { path: 'callsign', contains: 'TEST' } },
      { any: [{ path: 'classification.domain', in: ['air', 'surface'] }, { path: 'speed_mps', gt: 3.5 }] },
      { all: [{ path: 'ext.flag', exists: false }, { path: 'kind', eq: 'lob' }] },
    ]
    for (const c of conditions) {
      const built = toBuilt(c)
      expect(built).not.toBeNull()
      expect(fromBuilt(built!, { speed_mps: 'number' })).toEqual(c)
    }
  })

  it('leaves to JSON what rows cannot show', () => {
    expect(toBuilt({ all: [{ any: [{ path: 'a', eq: 1 }] }] })).toBeNull()
    expect(toBuilt({ not: { path: 'a', eq: 1 } })).toBeNull()
    expect(toBuilt({ value: { path: 'a', transforms: ['lower'] }, eq: 'x' })).toBeNull()
    expect(toBuilt({ path: 'a', gt: 1, lt: 5 })).toBeNull()
    expect(toBuilt(undefined)).toEqual({ match: 'all', rules: [] })
  })

  it('types values as the samples hold them, and skips incomplete rows', () => {
    expect(fromRule({ path: 'mmsi', op: 'eq', value: '366000001' }, { mmsi: 'number' })).toEqual({ path: 'mmsi', eq: 366000001 })
    expect(fromRule({ path: 'mmsi', op: 'eq', value: '366000001' }, { mmsi: 'string' })).toEqual({ path: 'mmsi', eq: '366000001' })
    expect(fromRule({ path: 'live', op: 'ne', value: 'true' }, { live: 'boolean' })).toEqual({ path: 'live', ne: true })
    expect(fromRule({ path: 'x', op: 'in', value: ' a, b ,,' })).toEqual({ path: 'x', in: ['a', 'b'] })
    expect(fromRule({ path: '', op: 'eq', value: 'a' })).toBeUndefined()
    expect(fromRule({ path: 'x', op: 'gt', value: 'abc' })).toBeUndefined()
    expect(fromBuilt({ match: 'any', rules: [{ path: 'x', op: 'starts_with', value: '' }] })).toBeUndefined()
  })

  it('lists the fields of sample observations with an example of each', () => {
    const fields = sampleFields([
      { source_track_key: 'tms-1', position: { lat: 1, lon: 2 }, callsign: null },
      { source_track_key: 'a', callsign: 'X', tags: ['p'] },
    ])
    expect(fields.map((f) => [f.path, f.kind, f.example])).toEqual([
      ['callsign', 'string', 'X'],
      ['position.lat', 'number', 1],
      ['position.lon', 'number', 2],
      ['source_track_key', 'string', 'tms-1'],
      ['tags', 'array', ['p']],
    ])
  })
})
