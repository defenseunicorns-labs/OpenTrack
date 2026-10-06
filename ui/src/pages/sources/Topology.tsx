import { useEffect, useMemo, useState } from 'react'
import '@xyflow/react/dist/base.css'
import { GraphView, type GraphViewEdge, type GraphViewNode } from '@kineticlogic/staresdk/graph-view'
import { api, type ServerStatus, type SourceRow, type SystemMetrics } from '../../api/client'
import { sourceState } from '../../lib/sourceState'

const REFRESH_MS = 10_000
const STAGE_W = 136
const END_W = 176

type Tone = GraphViewNode['tone']

/** Pipeline stages in the order the source worker runs them (ot-source `Pipeline::process`). */
function stages(s: SourceRow, recent: Record<string, number>, per: (n: number) => string) {
  const p = s.spec.pipeline
  const drop = (n: number, what: string) => (n > 0 ? `${per(n)} ${what}` : undefined)
  const out: { key: string; label: string; sublabel?: string; tone?: Tone }[] = [
    {
      key: 'decode',
      label: `decode ${p.codec.type}`,
      sublabel: drop(recent.decode_error ?? 0, 'errors') ?? `${per(recent.frames ?? 0)} in`,
      tone: (recent.decode_error ?? 0) > 0 ? 'warning' : undefined,
    },
    {
      key: 'map',
      label: 'map',
      sublabel: drop((recent.rejected ?? 0) + (recent.unmatched ?? 0), 'dropped') ?? `${p.mapping.rules.length} rules`,
    },
  ]
  if (p.static_join) out.push({ key: 'join', label: 'identity join' })
  // The registry stage runs on every source (with defaults when not configured).
  out.push({ key: 'registry', label: 'registry' })
  if (p.affiliation) out.push({ key: 'affiliation', label: 'affiliation' })
  if (p.filter) out.push({ key: 'filter', label: 'filter', sublabel: drop(recent.filtered ?? 0, 'dropped') })
  if (p.throttle) out.push({ key: 'throttle', label: 'throttle', sublabel: drop(recent.throttled ?? 0, 'held') })
  // What leaves the chain, on the last stage unless it reports its own drops.
  const last = out[out.length - 1]
  last.sublabel ??= `${per(recent.emitted ?? 0)} out`
  return out
}

const SOURCE_TONE: Record<string, Tone> = {
  disabled: 'muted',
  starting: 'warning',
  connecting: 'warning',
  failing: 'danger',
  running: 'neutral',
}

/**
 * Every source's processing chain, left to right, into correlation and out to NATS: the tracks
 * stream and any per-source raw feed. Moving dashes mark links that carried data in the last few
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
      let prev = `src:${s.id}`
      let flowing = recent.frames ?? 0
      for (const stage of stages(s, recent, per)) {
        const id = `${s.id}:${stage.key}`
        nodes.push({ id, label: stage.label, sublabel: stage.sublabel, tone: off ? 'muted' : stage.tone, width: STAGE_W })
        link(prev, id, flowing, off)
        prev = id
        flowing = recent.emitted ?? 0
      }
      link(prev, 'engine', recent.emitted ?? 0, off)
      if (s.raw_subject) {
        const raw = `raw:${s.id}`
        nodes.push({
          id: raw,
          label: s.raw_subject,
          sublabel: `raw feed · ${per(recent.raw_published ?? 0)}`,
          width: END_W,
          tone: off ? 'muted' : (recent.raw_error ?? 0) > 0 ? 'warning' : undefined,
        })
        link(prev, raw, recent.raw_published ?? 0, off)
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
        // Source, raw-feed and stage nodes open their source; shared nodes do nothing.
        const source = /^(src|raw):/.test(id) ? id.slice(4) : id.includes(':') ? id.slice(0, id.lastIndexOf(':')) : null
        if (source) onSelect(source)
      }}
    />
  )
}
