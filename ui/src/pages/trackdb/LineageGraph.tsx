import { useMemo } from 'react'
import '@xyflow/react/dist/base.css'
import { GraphView, type GraphViewEdge, type GraphViewNode } from '@kineticlogic/staresdk/graph-view'
import type { GraphEdge } from '../../api/client'

const UID_RE = /^[A-Z0-9]{3}\d{9}$/

/** A track's lineage from its graph edges: the source tracks reporting for it (current and
 *  past), the tracker that formed a source track from its source's detections, system tracks
 *  merged into or split from it, and the entity it resolves to. */
export default function LineageGraph({ uid, edges, entityId }: { uid: string; edges: GraphEdge[]; entityId?: string }) {
  const graph = useMemo(() => {
    const nodes = new Map<string, GraphViewNode>()
    const add = (key: string) => {
      if (nodes.has(key)) return
      const system = UID_RE.test(key)
      nodes.set(key, {
        id: key,
        label: system ? `tms-${key}` : key,
        sublabel: system ? 'system track' : 'source track',
        tone: key === uid ? 'accent' : 'neutral',
      })
    }
    const out: GraphViewEdge[] = []
    for (const e of edges) {
      add(e.src_key)
      add(e.dst_key)
      out.push({
        id: String(e.id),
        source: e.src_key,
        target: e.dst_key,
        label: e.kind,
        ended: e.valid_to_ms !== null,
      })
    }
    // Detections → tracker → source track, for source tracks a tracker formed.
    for (const e of edges) {
      const tracker = e.src_attrs?.tracker
      if (typeof tracker !== 'string' || UID_RE.test(e.src_key)) continue
      const source = e.src_key.split('/')[0]
      const id = `tracker:${source}:${tracker}`
      if (!nodes.has(id)) {
        nodes.set(id, { id, label: tracker, sublabel: `tracker · ${source} detections`, tone: 'neutral' })
      }
      const edgeId = `${id}>${e.src_key}`
      if (!out.some((x) => x.id === edgeId)) out.push({ id: edgeId, source: id, target: e.src_key, label: 'FORMED' })
    }
    // A source track whose every link has ended no longer reports for anything.
    for (const n of nodes.values()) {
      if (n.id !== uid && !n.id.startsWith('tracker:') && out.filter((e) => e.source === n.id).every((e) => e.ended)) n.tone = 'muted'
    }
    if (entityId) {
      nodes.set(`entity:${entityId}`, { id: `entity:${entityId}`, label: entityId, sublabel: 'entity' })
      out.push({ id: 'entity', source: uid, target: `entity:${entityId}`, label: 'ENTITY' })
    }
    return { nodes: [...nodes.values()], edges: out }
  }, [uid, edges, entityId])

  return <GraphView aria-label="Track lineage" nodes={graph.nodes} edges={graph.edges} height={260} />
}
