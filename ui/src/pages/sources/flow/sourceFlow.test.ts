import { describe, expect, it } from 'vitest'
import type { PluginInfo, SourceSpec } from '../../../api/client'
import {
  canConnect,
  COLUMN,
  defaultConfig,
  flowOf,
  freePlace,
  guessTarget,
  idFromName,
  joinDropped,
  laidOut,
  mapField,
  newSourceSpec,
  nextNodeId,
  nodeTitle,
  problemsByNode,
  validId,
  withCodec,
  withFlow,
  withoutPositions,
  withTransport,
  type Flow,
} from './sourceFlow'

const plugin = (name: string, framing: Record<string, unknown> | null = null): PluginInfo =>
  ({ name, kinds: ['codec'], enabled: true, default_options: { a: 1 }, framing }) as unknown as PluginInfo

/** input → map → { filter → output, nats }, and a throttle on its own. */
const branched = (): Flow => ({
  nodes: [
    { id: 'input', type: 'input' },
    { id: 'map', type: 'map', config: defaultConfig('map') },
    { id: 'filter', type: 'filter', config: {} },
    { id: 'output', type: 'output' },
    { id: 'nats', type: 'nats', config: { subject: 'raw' } },
    { id: 'throttle', type: 'throttle', config: defaultConfig('throttle') },
  ],
  edges: [
    { source: 'input', target: 'map' },
    { source: 'map', target: 'filter' },
    { source: 'filter', target: 'output' },
    { source: 'map', target: 'nats' },
  ],
})

describe('a new source', () => {
  it('has its input, an empty Map and the Output, connected and placed', () => {
    const s = newSourceSpec('ais', 'AIS', 'websocket')
    const f = flowOf(s)
    expect(f.nodes.map((n) => n.type)).toEqual(['input', 'map', 'output'])
    expect(f.edges).toEqual([
      { source: 'input', target: 'map' },
      { source: 'map', target: 'output' },
    ])
    expect(f.nodes.every((n) => n.position)).toBe(true)
    expect(s.transport).toEqual({ type: 'websocket', url: '' })
    expect(nodeTitle(s, f.nodes[0])).toBe('WebSocket')
  })

  it('takes its id from its name', () => {
    expect(idFromName('adsb.lol feed (EU)')).toBe('adsb-lol-feed-eu')
    expect(validId(idFromName('adsb.lol feed (EU)'))).toBe(true)
    expect(validId('Bad Id')).toBe(false)
  })
})

describe('canConnect', () => {
  const f = branched()

  it('lets a step that passes observations on feed one that takes them', () => {
    expect(canConnect(f, 'filter', 'throttle')).toBe(true)
    expect(canConnect(f, 'map', 'throttle')).toBe(true)
  })

  it('refuses the wrong data, a second input, a loop, and anything out of an output', () => {
    expect(canConnect(f, 'input', 'throttle')).toBe(false) // records into an observation step
    expect(canConnect(f, 'filter', 'map')).toBe(false) // observations into a Map
    expect(canConnect(f, 'map', 'output')).toBe(false) // the Output has its input
    expect(canConnect(f, 'output', 'throttle')).toBe(false) // an output passes nothing on
    expect(canConnect(f, 'throttle', 'throttle')).toBe(false)
    const chain: Flow = { ...f, edges: f.edges.filter((e) => e.target !== 'filter').concat({ source: 'throttle', target: 'filter' }) }
    // filter feeds output; throttle feeds filter: output → … → throttle would loop, filter → throttle too.
    expect(canConnect(chain, 'filter', 'throttle')).toBe(false)
  })
})

describe('steps', () => {
  it('are numbered by type when taken', () => {
    expect(nextNodeId('filter', ['input', 'map'])).toBe('filter')
    expect(nextNodeId('filter', ['filter', 'filter-2'])).toBe('filter-3')
  })

  it('without places are laid out in columns from the input', () => {
    const f = laidOut(branched())
    const at = (id: string) => f.nodes.find((n) => n.id === id)!.position!
    expect(at('input')).toEqual({ x: 0, y: 0 })
    expect(at('map').x).toBe(COLUMN)
    expect(at('filter').x).toBe(2 * COLUMN)
    expect(at('nats').x).toBe(2 * COLUMN)
    expect(at('filter').y).not.toBe(at('nats').y)
    expect(at('throttle').x).toBe(4 * COLUMN) // not connected: after the rest
  })

  it('are placed clear of the others', () => {
    const f = laidOut(branched())
    expect(freePlace(f, { x: 0, y: 0 })).toEqual({ x: 0, y: 130 })
    expect(freePlace(f, { x: 0.4, y: 500 })).toEqual({ x: 0, y: 500 })
  })

  it('are dry-run without their places', () => {
    const s = withFlow(newSourceSpec('x', 'X', 'udp'), laidOut(branched()))
    expect(flowOf(withoutPositions(s)).nodes.some((n) => n.position)).toBe(false)
  })
})

describe('the input', () => {
  const base = newSourceSpec('x', 'X', 'tcp_client')

  it('swaps its transport, and the acceptance that belonged to the old one goes', () => {
    const accepted: SourceSpec = { ...withTransport(base, 'udp'), unauthenticated: 'accepted' }
    const mqtt = withTransport(accepted, 'mqtt')
    expect(mqtt.transport).toEqual({ type: 'mqtt', url: '', topics: [] })
    expect(mqtt.unauthenticated).toBeUndefined()
    expect(withTransport(accepted, 'udp')).toBe(accepted)
  })

  it('frames CoT on a stream by its end tag, and takes it away again', () => {
    const cot = withCodec(base, 'cot_xml', [])
    expect(cot.pipeline.codec).toEqual({ type: 'cot_xml' })
    expect(cot.transport.framing).toEqual({ type: 'end_tag', tag: '</event>' })
    expect(withCodec(cot, 'json', []).transport.framing).toEqual({ type: 'lines' })
    const custom = { ...cot, transport: { ...cot.transport, framing: { type: 'delimiter', delimiter: '\\x03' } } }
    expect(withCodec(custom, 'json', []).transport.framing).toEqual(custom.transport.framing)
  })

  it('takes a codec plugin with its options and framing', () => {
    const p = plugin('stanag4607', { type: 'length_field', offset: 2, width: 'u32' })
    const s = withCodec(base, 'plugin:stanag4607', [p])
    expect(s.pipeline.codec).toEqual({ type: 'plugin', plugin: 'stanag4607', options: { a: 1 } })
    expect(s.transport.framing).toEqual({ type: 'length_field', offset: 2, width: 'u32' })
    expect(withCodec(s, 'json', [p]).transport.framing).toEqual({ type: 'lines' })
  })
})

describe('problemsByNode', () => {
  it('puts each problem on its step, the rest with the feed', () => {
    const p = problemsByNode(
      [
        { kind: 'incomplete', node: 'input', field: 'host', message: 'the host is not set' },
        { kind: 'incomplete', node: 'map', message: 'mapping has no rules' },
        { kind: 'incomplete', node: 'gone', message: 'not shown' },
        { kind: 'invalid', node: null, field: 'name', message: 'source name must not be empty' },
      ],
      branched(),
    )
    expect(p.byNode).toEqual({ input: ['the host is not set'], map: ['mapping has no rules'] })
    expect(p.general).toEqual(['not shown', 'source name must not be empty'])
  })
})

describe('joinDropped', () => {
  // input → map → output, in a row 300 apart.
  const line = (): Flow => ({
    nodes: [
      { id: 'input', type: 'input', position: { x: 0, y: 0 } },
      { id: 'map', type: 'map', config: defaultConfig('map'), position: { x: 300, y: 0 } },
      { id: 'output', type: 'output', position: { x: 600, y: 0 } },
    ],
    edges: [
      { source: 'input', target: 'map' },
      { source: 'map', target: 'output' },
    ],
  })
  const drop = (f: Flow, n: Flow['nodes'][number]) => joinDropped({ ...f, nodes: [...f.nodes, n] }, n.id)

  it('puts a step dropped on a connection between its two steps', () => {
    const f = drop(line(), { id: 'filter', type: 'filter', config: {}, position: { x: 420, y: 10 } })
    expect(f.edges).toEqual([
      { source: 'input', target: 'map' },
      { source: 'map', target: 'filter' },
      { source: 'filter', target: 'output' },
    ])
  })

  it('does not put a step on a connection carrying the wrong data', () => {
    // A Filter takes observations; input → map carries records.
    const f = drop(line(), { id: 'filter', type: 'filter', config: {}, position: { x: 110, y: 0 } })
    expect(f.edges.some((e) => e.target === 'filter' && e.source === 'input')).toBe(false)
  })

  it('connects a step dropped just right of an output that can feed it', () => {
    const f = drop(line(), { id: 'nats', type: 'nats', config: { subject: 'x' }, position: { x: 560, y: 70 } })
    expect(f.edges).toContainEqual({ source: 'map', target: 'nats' })
  })

  it('leaves a step dropped far from everything loose', () => {
    const f = drop(line(), { id: 'throttle', type: 'throttle', config: {}, position: { x: 1400, y: 900 } })
    expect(f.edges).toEqual(line().edges)
  })
})

describe('a field dropped on a Map', () => {
  it('goes where the capture proposed, else to an extension field named after it', () => {
    const proposals = [
      { target: 'kinematics.speed_mps', spec: { path: 'body.velocity', transforms: ['knots'] }, confidence: 0.9, reason: '' },
      { target: 'position.latitude', spec: 'body.y', confidence: 0.8, reason: '' },
      { target: 'key', spec: 'body.ref', confidence: 0.7, reason: '' },
    ]
    expect(guessTarget('body.velocity', proposals)).toBe('kinematics.speed_mps')
    expect(guessTarget('body.y', proposals)).toBe('position.latitude')
    expect(guessTarget('body.ref', proposals)).toBe('key')
    // No proposal: never guessed from a name.
    expect(guessTarget('body.latitude')).toBe('ext.latitude')
    expect(guessTarget('record.StatusCode')).toBe('ext.status_code')
    expect(guessTarget('event.@ref')).toBe('ext.ref')
  })

  it("lands in the Map's first reporting rule, which is started if it has none", () => {
    const fresh = mapField({ mapping: { rules: [] } }, 'lat', 'position.latitude') as { mapping: { rules: unknown[] } }
    expect(fresh.mapping.rules).toEqual([{ name: 'main', key: '', fields: { 'position.latitude': 'lat' } }])
    const keyed = mapField(fresh, 'id', 'key') as { mapping: { rules: { key: string }[] } }
    expect(keyed.mapping.rules[0].key).toBe('id')
    const withStatic = mapField(
      { mapping: { rules: [{ name: 's', kind: 'static', key: 'id', fields: {} }, { name: 'p', key: 'id', fields: { name: 'n' } }] } },
      'lon',
      'position.longitude',
    ) as { mapping: { rules: { fields: Record<string, string> }[] } }
    expect(withStatic.mapping.rules[1].fields).toEqual({ name: 'n', 'position.longitude': 'lon' })
    expect(withStatic.mapping.rules[0].fields).toEqual({})
  })
})
