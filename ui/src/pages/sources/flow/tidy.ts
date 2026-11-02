import dagre from '@dagrejs/dagre'
import type { Flow } from './sourceFlow'

/** A step card's size on the workspace, roughly (240 wide, up to five lines tall). */
const STEP_W = 240
const STEP_H = 120

/** The flow laid out left to right in layers, the input first, branches side by side. */
export function tidy(flow: Flow): Flow {
  const g = new dagre.graphlib.Graph()
  g.setGraph({ rankdir: 'LR', nodesep: 40, ranksep: 80, marginx: 0, marginy: 0 })
  g.setDefaultEdgeLabel(() => ({}))
  for (const n of flow.nodes) g.setNode(n.id, { width: STEP_W, height: STEP_H })
  for (const e of flow.edges) g.setEdge(e.source, e.target)
  dagre.layout(g)
  return {
    ...flow,
    nodes: flow.nodes.map((n) => {
      const p = g.node(n.id)
      return p ? { ...n, position: { x: Math.round(p.x - STEP_W / 2), y: Math.round(p.y - STEP_H / 2) } } : n
    }),
  }
}
