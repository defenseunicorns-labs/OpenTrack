import { describe, expect, it } from 'vitest'
import type { SchemaOverview, SourceSpec } from '../api/client'
import { describeCondition, describeValue, destinations, fieldMap, pipelineStages, transportCodec, valueSources } from './pipeline'

const schema = {
  published_core: [],
  core: [],
  reserved_extension_keys: ['registry'],
  latest_published: 2,
  sources: [],
  builtins: [
    { name: 'platform_type', type: 'string', reads: ['platform.type_code'] },
    { name: 'callsign', type: 'string', reads: ['callsign'] },
  ],
  gold_sources: { name: ['name', 'platform.name'], lat: ['position.latitude'] },
  versions: [
    {
      version: 2,
      status: 'published',
      published_at_ms: 0,
      notes: null,
      fields: [
        { key: 'callsign', type: 'string', builtin: 'callsign' },
        { key: 'registration', type: 'string' },
        { key: 'state', type: 'string', builtin: 'state' },
      ],
    },
  ],
} as unknown as SchemaOverview

describe('describe', () => {
  it('reads value specs and conditions in plain words', () => {
    expect(describeValue({ path: 'gs', transforms: ['knots_to_mps'] })).toBe('gs · knots_to_mps')
    expect(describeValue({ first: ['flight', 'r'] })).toBe('first of flight, r')
    expect(describeValue({ path: 't', table: { A: 1, B: 2 }, table_default: 'x' })).toBe('t · look up in a 2 entries table · else "x"')
    expect(describeCondition({ all: [{ path: 'lat', exists: true }, { not: { path: 'f', matches: '^T' } }] })).toBe(
      'lat is present and not (f matches "^T")',
    )
    expect(valueSources({ template: '{a}-{b.c}', default: 'x' })).toEqual(['a', 'b.c'])
  })
})

describe('destinations', () => {
  it('traces feed values to where they are published', () => {
    expect(destinations('ext.registration', schema)).toEqual([{ kind: 'attribute', text: 'attributes.registration' }])
    expect(destinations('ext.squawk', schema)[0].kind).toBe('none')
    expect(destinations('ext.state', schema)[0].text).toContain('built-in')
    expect(destinations('callsign', schema)).toEqual([{ kind: 'attribute', text: 'attributes.callsign' }])
    expect(destinations('name', schema)).toEqual([{ kind: 'gold', text: 'name' }])
    // The type_code case: a built-in reads it, but no schema field links that built-in.
    const t = destinations('platform.type_code', schema)
    expect(t).toHaveLength(1)
    expect(t[0].kind).toBe('none')
    expect(t[0].text).toContain('platform_type')
    expect(destinations('provenance.source_code', schema)[0].kind).toBe('internal')
  })

  it('lists every value a pipeline sets', () => {
    const spec = {
      id: 's',
      name: 's',
      transport: { type: 'http_poll' },
      pipeline: {
        codec: { type: 'json' },
        mapping: {
          rules: [
            {
              name: 'position',
              key: 'hex',
              identifiers: [{ scheme: 'icao', value: 'hex' }],
              fields: { callsign: 'flight', 'platform.type_code': 't' },
            },
          ],
        },
        registry: { apply: { 'platform.name': 'name' } },
      },
    } as unknown as SourceSpec
    const rows = fieldMap(spec, schema)
    expect(rows.map((r) => r.target)).toEqual(['track key', 'identifier icao', 'callsign', 'platform.type_code', 'platform.name'])
    expect(rows.find((r) => r.target === 'platform.name')!.stage).toBe('entity')
  })
})

describe('pipelineStages', () => {
  it('shows a tracker stage before publishing, with the classification it may give', () => {
    const spec = {
      id: 'radar',
      name: 'Radar',
      transport: { type: 'udp' },
      reports: 'detections',
      pipeline: {
        codec: { type: 'json' },
        mapping: { rules: [{ name: 'plot', key: 'id', fields: {} }] },
        tracker: { algorithm: 'mht', domain: 'surface' },
      },
    } as unknown as SourceSpec
    const stages = pipelineStages(spec)
    const ids = stages.map((s) => s.id)
    expect(ids.indexOf('tracker')).toBe(ids.length - 2)
    const tracker = stages.find((s) => s.id === 'tracker')!
    expect(tracker.title).toBe('tracker · MHT')
    expect(tracker.facts.find((f) => f.label === 'Classification')!.value).toBe('unknown affiliation, surface; no identity')
    // With a tracker the feed's plots arrive as tracks.
    expect(stages.at(-1)!.summary).toBe('system track, published to NATS')
  })
})

describe('transportCodec', () => {
  it('leaves the codecs the Decode stage edits to it', () => {
    expect(transportCodec(undefined)).toBeNull()
    expect(transportCodec({ type: 'json', records: 'ac' })).toBeNull()
    expect(transportCodec({ type: 'cot_xml' })).toBeNull()
    expect(transportCodec({ type: 'xml', record_element: 'r' })).toBeNull()
  })

  it('shows protobuf and plugin codecs with their message or plugin', () => {
    expect(transportCodec({ type: 'protobuf', files: {}, message: 'acme.v1.Batch' })).toEqual({ type: 'protobuf', name: 'acme.v1.Batch' })
    expect(transportCodec({ type: 'protobuf', files: {}, message: '' })).toEqual({ type: 'protobuf' })
    expect(transportCodec({ type: 'plugin', plugin: 'sapient', options: {} })).toEqual({ type: 'plugin', name: 'sapient' })
    expect(transportCodec({ type: 'future_codec' })).toEqual({ type: 'future_codec' })
  })
})
