import { useEffect, useMemo, useState } from 'react'
import '@xyflow/react/dist/base.css'
import { GraphView, type GraphViewEdge, type GraphViewNode } from '@kineticlogic/staresdk/graph-view'
import { api, type ServerStatus, type SourceRow, type SystemMetrics } from '../../api/client'
import { sourceState } from '../../lib/sourceState'
import { flowOf, INPUT, nodeTitle } from './flow/sourceFlow'

const REFRESH_MS = 10_000
const STAGE_W = 136
const END_W = 176

type Tone = GraphViewNode['tone']

const SOURCE_TONE: Record<string, Tone> = {
  disabled: 'muted',
  starting: 'warning',
  connecting: 'warning',
  failing: 'danger',
  running: 'neutral',
}

/**
 * Every source's flow, left to right, into correlation and out to NATS: the tracks stream and
 * each source's NATS topics. Moving dashes mark links that carried data in the last few
 * minutes; dashed grey links belong to disabled sources.
 */
export default function Topology({ sources, onSelect }: { sources: SourceRow[]; onSelect: (id: string) => void }) {
  const [metrics, setMetrics] = useState<SystemMetrics | null>(null)
  const [status, setStatus] = useState<ServerStatus | null>(null)

  useEffect(() => {
    let cancelled = false
    const load = () => {
      api.systemMetrics(10).then((m) => !cancelled && setMetrics(m), () => {})
      api.status().then((s) => !cancelled && setStatus(s), () => {})
    }
    load()
    const t = setInterval(load, REFRESH_MS)
    return () => {
      cancelled = true
      clearInterval(t)
    }
  }, [])

  const graph = useMemo(() => {
    const minutes = metrics?.recent_minutes ?? 5
    const rate = (n: number) => n / minutes
    const per = (n: number) => {
      const r = rate(n)
      return r === 0 ? '0/min' : r < 1 ? '<1/min' : `${Math.round(r).toLocaleString()}/min`
    }
    const nodes: GraphViewNode[] = []
    const edges: GraphViewEdge[] = []
    // No edge labels: a label widens every layer gap, and rates read fine on the nodes.
    const link = (source: string, target: string, n: number, off: boolean) =>
      edges.push({ id: `${source}>${target}`, source, target, animated: n > 0, ended: off })

    for (const s of sources) {
      const recent = metrics?.recent.sources[s.id] ?? {}
      const st = sourceState(s)
      const off = !s.enabled
      nodes.push({
        id: `src:${s.id}`,
        label: s.name,
        sublabel: `${s.transport} · ${st.label}`,
        tone: SOURCE_TONE[st.label],
        width: END_W,
      })
      // The source's flow: its steps after the input, the Output into correlation, each NATS
      // topic out on its own.
      const flow = flowOf(s.spec)
      const at = (id: string) => (id === INPUT ? `src:${s.id}` : `${s.id}:${id}`)
      for (const n of flow.nodes) {
        if (n.type === 'input' || n.type === 'output') continue
        const nats = n.type === 'nats'
        nodes.push({
          id: at(n.id),
          label: nodeTitle(s.spec, n),
          sublabel: nats ? 'NATS topic' : undefined,
          tone: off ? 'muted' : nats && (recent.publish_error ?? 0) > 0 ? 'warning' : undefined,
          width: nats ? END_W : STAGE_W,
        })
      }
      for (const e of flow.edges) {
        const to = flow.nodes.find((n) => n.id === e.target)
        if (to?.type === 'output') link(at(e.source), 'engine', recent.emitted ?? 0, off)
        else link(at(e.source), at(e.target), to?.type === 'nats' ? (recent.published ?? 0) : (recent.records ?? 0), off)
      }
    }

    const live = metrics?.live
    const engine = metrics?.recent.engine ?? {}
    const writer = metrics?.recent.writer ?? {}
    nodes.push({
      id: 'engine',
      label: 'Correlation',
      sublabel: live ? `${live.tracks.toLocaleString()} tracks · ${per(engine.observations ?? 0)}` : 'engine',
      tone: 'accent',
      width: END_W,
    })
    const backlog = live ? live.outbox.lag + live.outbox.pending : 0
    nodes.push({
      id: 'writer',
      label: 'Track writer',
      sublabel: live ? `${per(writer.written ?? 0)} · ${backlog.toLocaleString()} waiting` : undefined,
      tone: backlog > 1000 ? 'warning' : undefined,
      width: END_W,
    })
    link('engine', 'writer', engine.observations ?? 0, false)
    const nats = status?.nats
    nodes.push({
      id: 'nats',
      label: `${nats?.stream ?? 'TRACKS'} stream`,
      sublabel: nats?.ok
        ? `${nats.tracks_subject}.> · ${(nats.stream_messages ?? 0).toLocaleString()} msgs`
        : nats
          ? 'NATS down'
          : undefined,
      tone: nats && !nats.ok ? 'danger' : undefined,
      width: END_W,
    })
    link('writer', 'nats', writer.written ?? 0, false)
    return { nodes, edges }
  }, [sources, metrics, status])

  const height = Math.max(240, sources.length * 70 + 90)
  return (
    <GraphView
      aria-label="Source topology"
      nodes={graph.nodes}
      edges={graph.edges}
      height={height}
      layerGap={36}
      onNodeClick={(id) => {
        // Source and step nodes open their source; shared nodes do nothing.
        const source = id.startsWith('src:') ? id.slice(4) : id.includes(':') ? id.slice(0, id.indexOf(':')) : null
        if (source) onSelect(source)
      }}
    />
  )
}
