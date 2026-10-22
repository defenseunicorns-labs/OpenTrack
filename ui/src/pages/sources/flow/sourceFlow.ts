/**
 * A source as a flow of steps (#104), as the server keeps it (crates/ot-source/src/flow.rs): one
 * input (the transport and its codec), then steps the user places and connects, each taking one
 * input and feeding any number of others, ending at the source's one Output (system tracks, to
 * correlation) and any NATS topics (its mapped data, without correlation).
 *
 * Records flow from the input through Reject steps to a Map, which makes observations; Filter,
 * Entity links, Affiliation, Tracker and Throttle steps take observations; Output and NATS topics
 * take them and pass nothing on.
 */
import type { PluginInfo, SourceProblem, SourceSpec } from '../../../api/client'
import { DEFAULT_LINKS } from '../../../lib/pipeline'

type Obj = Record<string, unknown>
type Transport = SourceSpec['transport']
type Codec = SourceSpec['pipeline']['codec']

export const INPUT = 'input'

export type StepType = 'input' | 'reject' | 'map' | 'filter' | 'registry' | 'affiliation' | 'tracker' | 'throttle' | 'output' | 'nats'

export interface FlowNode {
  id: string
  type: StepType
  config?: unknown
  position?: { x: number; y: number }
}

export interface FlowEdge {
  source: string
  target: string
}

/** A flow's steps and connections, as the spec's `pipeline` holds them. */
export interface Flow {
  nodes: FlowNode[]
  edges: FlowEdge[]
}

type Data = 'records' | 'observations'

/** What each step takes and passes on (`null`: nothing). */
const DATA: Record<StepType, { takes: Data | null; gives: Data | null }> = {
  input: { takes: null, gives: 'records' },
  reject: { takes: 'records', gives: 'records' },
  map: { takes: 'records', gives: 'observations' },
  filter: { takes: 'observations', gives: 'observations' },
  registry: { takes: 'observations', gives: 'observations' },
  affiliation: { takes: 'observations', gives: 'observations' },
  tracker: { takes: 'observations', gives: 'observations' },
  throttle: { takes: 'observations', gives: 'observations' },
  output: { takes: 'observations', gives: null },
  nats: { takes: 'observations', gives: null },
}

export const takes = (t: StepType) => DATA[t].takes
export const gives = (t: StepType) => DATA[t].gives

/** Each step's name, and the group the palette lists it under. */
export const STEPS: Record<Exclude<StepType, 'input'>, { label: string; group: 'Records' | 'Observations' | 'Outputs' }> = {
  reject: { label: 'Reject', group: 'Records' },
  map: { label: 'Map', group: 'Records' },
  filter: { label: 'Filter', group: 'Observations' },
  registry: { label: 'Entity links', group: 'Observations' },
  affiliation: { label: 'Affiliation', group: 'Observations' },
  tracker: { label: 'Tracker', group: 'Observations' },
  throttle: { label: 'Throttle', group: 'Observations' },
  output: { label: 'Output', group: 'Outputs' },
  nats: { label: 'NATS topic', group: 'Outputs' },
}

/** The word under a step's heading. */
export const KIND_WORD: Record<StepType, string> = {
  input: 'Input',
  reject: 'Records',
  map: 'Map',
  filter: 'Observations',
  registry: 'Observations',
  affiliation: 'Observations',
  tracker: 'Observations',
  throttle: 'Observations',
  output: 'Correlation',
  nats: 'NATS',
}

/** What a new step starts as. */
export function defaultConfig(type: StepType): unknown {
  switch (type) {
    case 'reject':
      return [{ reason: 'no_position', when: { path: 'lat', exists: false } }]
    case 'map':
      return { mapping: { rules: [] } }
    case 'filter':
      return {}
    case 'registry':
      return { links: DEFAULT_LINKS }
    case 'affiliation':
      return { country: 'platform.flag', friendly: [], hostile: [], neutral: [] }
    case 'tracker':
      return { algorithm: 'gnn' }
    case 'throttle':
      return { min_interval_secs: 5, min_move_m: 0, heartbeat_secs: 600 }
    case 'nats':
      return { subject: '' }
    default:
      return undefined
  }
}

/** Transports, as the input's settings name them, with what a new one starts as. */
export const TRANSPORTS: Record<string, { label: string; initial: Transport }> = {
  tcp_client: { label: 'TCP client', initial: { type: 'tcp_client', host: '', port: 0, framing: { type: 'lines' } } },
  tcp_server: { label: 'TCP server', initial: { type: 'tcp_server', bind: '0.0.0.0:8087', framing: { type: 'lines' } } },
  udp: { label: 'UDP', initial: { type: 'udp', bind: '0.0.0.0:6969' } },
  http_poll: { label: 'HTTP poll', initial: { type: 'http_poll', url: '', interval_secs: 5 } },
  websocket: { label: 'WebSocket', initial: { type: 'websocket', url: '' } },
  mqtt: { label: 'MQTT', initial: { type: 'mqtt', url: '', topics: [] } },
  grpc_client: { label: 'gRPC client', initial: { type: 'grpc_client', url: '', method: '' } },
  grpc_server: { label: 'gRPC server', initial: { type: 'grpc_server', bind: '0.0.0.0:50051' } },
  file: { label: 'File', initial: { type: 'file', path: '' } },
}

/** Codecs, likewise. A plugin codec is offered by its plugin's name. */
export const CODECS: Record<string, { label: string; initial: Codec }> = {
  json: { label: 'JSON', initial: { type: 'json' } },
  cot_xml: { label: 'CoT XML', initial: { type: 'cot_xml' } },
  xml: { label: 'XML', initial: { type: 'xml', record_element: '' } },
  protobuf: { label: 'Protobuf', initial: { type: 'protobuf', files: {}, message: '' } },
}

/** The codecs the input can use: the built-in ones and each enabled codec plugin, as `plugin:<name>`. */
export function codecChoices(plugins: PluginInfo[]): { value: string; label: string }[] {
  return [
    ...Object.entries(CODECS).map(([value, c]) => ({ value, label: c.label })),
    ...plugins.filter((p) => p.kinds.includes('codec') && p.enabled).map((p) => ({ value: `plugin:${p.name}`, label: p.name })),
  ]
}

export const codecChoice = (c: Codec) => (c.type === 'plugin' ? `plugin:${String(c.plugin ?? '')}` : c.type)

const streamed = (t: Transport) => t.type === 'tcp_client' || t.type === 'tcp_server'
const LINES = { type: 'lines' }
const COT_FRAMING = { type: 'end_tag', tag: '</event>' }
const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)

/** CoT arrives as back-to-back XML events, framed by `</event>` rather than by lines on a stream. */
function cotFraming(t: Transport, codec: Codec): Transport {
  const framing = t.framing as { type?: string } | undefined
  return codec.type === 'cot_xml' && streamed(t) && framing?.type === 'lines' ? { ...t, framing: COT_FRAMING } : t
}

/** The framing a codec sets on a stream when chosen (CoT's end tag, a plugin's own), if any. */
function framingOf(codec: Codec, plugins: PluginInfo[]): unknown {
  if (codec.type === 'cot_xml') return COT_FRAMING
  if (codec.type === 'plugin') return plugins.find((p) => p.name === codec.plugin)?.framing ?? null
  return null
}

/** The source with another transport: its defaults, framed for the codec; the risk acceptance goes with the old one. */
export function withTransport(spec: SourceSpec, type: string): SourceSpec {
  const d = TRANSPORTS[type]
  if (!d || type === spec.transport.type) return spec
  const next: SourceSpec = { ...spec, transport: cotFraming(structuredClone(d.initial), spec.pipeline.codec) }
  delete next.unauthenticated
  return next
}

/** The source with another codec (`plugin:<name>` for a plugin's): the framing the old one set goes, the new one's comes. */
export function withCodec(spec: SourceSpec, choice: string, plugins: PluginInfo[]): SourceSpec {
  if (choice === codecChoice(spec.pipeline.codec)) return spec
  let codec: Codec | null = null
  let transport = spec.transport
  const before = framingOf(spec.pipeline.codec, plugins)
  if (before && streamed(transport) && same(transport.framing, before)) transport = { ...transport, framing: LINES }
  if (choice.startsWith('plugin:')) {
    const p = plugins.find((x) => x.name === choice.slice('plugin:'.length))
    if (p) {
      codec = { type: 'plugin', plugin: p.name, options: structuredClone(p.default_options) }
      if (p.framing && streamed(transport)) transport = { ...transport, framing: structuredClone(p.framing) }
    }
  } else if (CODECS[choice]) {
    codec = structuredClone(CODECS[choice].initial)
  }
  if (!codec) return spec
  return { ...spec, transport: cotFraming(transport, codec), pipeline: { ...spec.pipeline, codec } }
}

/** The source's flow. */
export const flowOf = (spec: SourceSpec): Flow => ({
  nodes: (spec.pipeline.nodes as FlowNode[] | undefined) ?? [],
  edges: (spec.pipeline.edges as FlowEdge[] | undefined) ?? [],
})

/** The source with this flow. */
export const withFlow = (spec: SourceSpec, flow: Flow): SourceSpec => ({ ...spec, pipeline: { ...spec.pipeline, nodes: flow.nodes, edges: flow.edges } })

/** A new step's id: its type, numbered when taken (`filter`, `filter-2`…). */
export function nextNodeId(type: StepType, ids: string[]): string {
  if (!ids.includes(type)) return type
  let n = 2
  while (ids.includes(`${type}-${n}`)) n++
  return `${type}-${n}`
}

/** Whether `source` may feed `target`: one input each, no loops, and the right data. */
export function canConnect(flow: Flow, source: string, target: string): boolean {
  if (source === target) return false
  const from = flow.nodes.find((n) => n.id === source)
  const to = flow.nodes.find((n) => n.id === target)
  if (!from || !to) return false
  const out = gives(from.type)
  if (!out || takes(to.type) !== out) return false
  if (flow.edges.some((e) => e.target === target)) return false
  // No loop: `target` must not already lead to `source`.
  const children = new Map<string, string[]>()
  for (const e of flow.edges) children.set(e.source, [...(children.get(e.source) ?? []), e.target])
  const stack = [target]
  const seen = new Set<string>()
  while (stack.length) {
    const at = stack.pop()!
    if (at === source) return false
    if (seen.has(at)) continue
    seen.add(at)
    stack.push(...(children.get(at) ?? []))
  }
  return true
}

/** Where steps go that have no place yet (a source from before flows): columns by distance from the input. */
export const COLUMN = 300
export const ROW = 130

export function laidOut(flow: Flow): Flow {
  if (flow.nodes.every((n) => n.position)) return flow
  const children = new Map<string, string[]>()
  for (const e of flow.edges) children.set(e.source, [...(children.get(e.source) ?? []), e.target])
  const depth = new Map<string, number>([[INPUT, 0]])
  const queue = [INPUT]
  while (queue.length) {
    const at = queue.shift()!
    for (const c of children.get(at) ?? []) {
      if (!depth.has(c)) {
        depth.set(c, depth.get(at)! + 1)
        queue.push(c)
      }
    }
  }
  const rows = new Map<number, number>()
  const loose = Math.max(0, ...depth.values()) + 1
  return {
    ...flow,
    nodes: flow.nodes.map((n) => {
      if (n.position) return n
      const col = depth.get(n.id) ?? loose
      const row = rows.get(col) ?? 0
      rows.set(col, row + 1)
      return { ...n, position: { x: col * COLUMN, y: row * ROW } }
    }),
  }
}

/** Where a new step goes: `at`, or below it as far as needed not to cover another step. */
export function freePlace(flow: Flow, at: { x: number; y: number }): { x: number; y: number } {
  const taken = (p: { x: number; y: number }) =>
    flow.nodes.some((n) => n.position && Math.abs(n.position.x - p.x) < 240 && Math.abs(n.position.y - p.y) < 110)
  let p = { x: Math.round(at.x), y: Math.round(at.y) }
  for (let i = 0; i < 50 && taken(p); i++) p = { ...p, y: p.y + ROW }
  return p
}

export interface NodeProblems {
  byNode: Record<string, string[]>
  /** Problems with the source as a whole (its feed settings), or with a step it no longer has. */
  general: string[]
}

/** The server's problems, each on its step. */
export function problemsByNode(problems: SourceProblem[], flow: Flow): NodeProblems {
  const ids = new Set(flow.nodes.map((n) => n.id))
  const out: NodeProblems = { byNode: {}, general: [] }
  for (const p of problems) {
    if (p.node && ids.has(p.node)) (out.byNode[p.node] ??= []).push(p.message)
    else out.general.push(p.message)
  }
  return out
}

/** A step's heading. */
export function nodeTitle(spec: SourceSpec, n: FlowNode): string {
  if (n.type === 'input') return TRANSPORTS[spec.transport.type]?.label ?? spec.transport.type
  if (n.type === 'nats') return String((n.config as Obj | undefined)?.subject || 'NATS topic')
  return STEPS[n.type].label
}

/** One line of a step's settings. */
export function nodeSummary(spec: SourceSpec, n: FlowNode): string {
  const c = (n.config ?? {}) as Obj
  switch (n.type) {
    case 'input': {
      const codec = spec.pipeline.codec
      const t = spec.transport
      const where = String(t.url ?? t.bind ?? t.path ?? (t.host ? `${t.host}:${t.port}` : ''))
      const decode = codec.type === 'plugin' ? String(codec.plugin ?? 'plugin') : (CODECS[codec.type]?.label ?? codec.type)
      return [where, decode].filter(Boolean).join(' · ')
    }
    case 'reject':
      return plural(((n.config as unknown[]) ?? []).length, 'rule')
    case 'map':
      return plural((((c.mapping as Obj | undefined)?.rules as unknown[]) ?? []).length, 'rule')
    case 'filter':
      return c.keep_if ? 'keep only matching' : c.drop_if ? 'drop matching' : 'keeps everything'
    case 'tracker':
      return String(c.algorithm ?? 'gnn').toUpperCase()
    case 'throttle':
      return `at most every ${String(c.min_interval_secs ?? 0)} s`
    case 'output':
      return spec.reports === 'detections' ? 'detections' : 'tracks'
    default:
      return ''
  }
}

const plural = (n: number, one: string) => `${n} ${n === 1 ? one : `${one}s`}`

/** A new source: its transport, a JSON input, an empty Map and the Output, connected. */
export function newSourceSpec(id: string, name: string, transport: string): SourceSpec {
  const flow: Flow = {
    nodes: [
      { id: INPUT, type: 'input', position: { x: 0, y: 0 } },
      { id: 'map', type: 'map', config: { mapping: { rules: [] } }, position: { x: COLUMN, y: 0 } },
      { id: 'output', type: 'output', position: { x: 2 * COLUMN, y: 0 } },
    ],
    edges: [
      { source: INPUT, target: 'map' },
      { source: 'map', target: 'output' },
    ],
  }
  return {
    id,
    name,
    transport: structuredClone(TRANSPORTS[transport]?.initial ?? TRANSPORTS.http_poll.initial),
    pipeline: { codec: { type: 'json' }, nodes: flow.nodes, edges: flow.edges },
  }
}

/** A source id from its name: lowercase letters, digits, `-` and `_`, at most 64 (crates/ot-source/src/source.rs). */
export function idFromName(name: string): string {
  return name
    .toLowerCase()
    .normalize('NFKD')
    .replace(/[^a-z0-9_-]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 64)
}

export const validId = (id: string) => /^[a-z0-9_-]{1,64}$/.test(id)

/** The spec without where its steps are drawn: what a dry run depends on. */
export function withoutPositions(spec: SourceSpec): SourceSpec {
  const flow = flowOf(spec)
  return withFlow(spec, {
    ...flow,
    nodes: flow.nodes.map((n) => {
      const copy = { ...n }
      delete copy.position
      return copy
    }),
  })
}
