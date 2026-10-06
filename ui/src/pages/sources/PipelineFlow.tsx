import { useMemo } from 'react'
import '@xyflow/react/dist/base.css'
import { GraphView, type GraphViewEdge, type GraphViewNode } from '@kineticlogic/staresdk/graph-view'
import type { Stage } from '../../lib/pipeline'

const NODE_W = 196
const ROW = 46 + 28

/** The stages as a top-to-bottom flowchart; click one to see its detail. */
export default function PipelineFlow({ stages, selected, onSelect }: { stages: Stage[]; selected: string; onSelect: (id: string) => void }) {
  const graph = useMemo(() => {
    const nodes: GraphViewNode[] = stages.map((s) => ({
      id: s.id,
      label: s.title,
      sublabel: s.summary,
      width: NODE_W,
      tone: s.id === 'publish' ? 'accent' : undefined,
    }))
    const edges: GraphViewEdge[] = stages.slice(1).map((s, i) => ({ id: `${stages[i].id}>${s.id}`, source: stages[i].id, target: s.id }))
    return { nodes, edges }
  }, [stages])
  return (
    <GraphView
      aria-label="Pipeline stages"
      direction="TB"
      nodes={graph.nodes}
      edges={graph.edges}
      layerGap={28}
      height={stages.length * ROW + 24}
      selectedId={selected}
      onNodeClick={onSelect}
    />
  )
}
