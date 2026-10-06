import { describe, expect, it } from 'vitest'
import { fromForm, toForm } from './valueSpec'

describe('value spec form', () => {
  it('round-trips the specs the form can hold', () => {
    const specs = [
      'Message.PositionReport.Sog',
      { path: 'flight', transforms: ['trim', 'nonempty'] },
      { first: ['flight', 'r', 'hex'], transforms: ['trim'] },
      { const: 'surface' },
      { const: 42 },
      { template: 'a-{aff}-A', default: 'a-u-A' },
      { path: 'heading', null_if: [511], transforms: ['wrap360'] },
      { path: 't', table: { B738: 'civil' }, table_default: 'unknown' },
      { path: 'x', transforms: ['upper', 'split: :0'] },
    ]
    for (const s of specs) {
      const f = toForm(s)
      expect(f.mode).not.toBe('advanced')
      expect(fromForm(f)).toEqual(s)
    }
  })

  it('leaves richer or ambiguous specs to JSON', () => {
    const advanced = [
      { cases: [{ when: { path: 'a', eq: 1 }, then: { const: 2 } }], default: 0 },
      { arith: 'sub', args: ['a', 'b'] },
      { path: 'Message', key: 'MessageType' },
      { path: 'hex', ranges: [[1, 2, 'US']] },
      { first: [{ path: 'a', transforms: ['trim'] }, 'b'] },
      { path: 'a', transforms: ['replace:,:;'] },
      // Its trailing space matters: it replaces @ with a space.
      { path: 'a', transforms: ['replace:@: ', 'trim'] },
      // A text "0" would come back as the number 0.
      { path: 'a', default: '0' },
      { path: 'a', null_if: ['true'] },
      { path: 'a', const: 1 },
    ]
    for (const s of advanced) expect(toForm(s).mode).toBe('advanced')
  })
})

import aisstream from '../../../docs/examples/aisstream.json'
import adsb from '../../../docs/examples/adsb-lol.json'

describe('real pipelines', () => {
  it('never changes a value the form can hold', () => {
    let checked = 0
    for (const spec of [aisstream, adsb] as { pipeline: { mapping: { rules: Record<string, unknown>[] } } }[]) {
      for (const rule of spec.pipeline.mapping.rules) {
        const values = [rule.key, ...Object.values((rule.fields as Record<string, unknown>) ?? {})]
        for (const id of (rule.identifiers as { value: unknown }[]) ?? []) values.push(id.value)
        for (const v of values) {
          const f = toForm(v as never)
          if (f.mode === 'advanced') continue
          expect(fromForm(f)).toEqual(v)
          checked++
        }
      }
    }
    expect(checked).toBeGreaterThan(20)
  })
})
