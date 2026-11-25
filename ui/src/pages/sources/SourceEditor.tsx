import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { TbDownload, TbLayoutDistributeVertical, TbList, TbSettings, TbTable, TbTrash, TbWand } from 'react-icons/tb'
import type { Connection, Edge, EdgeChange, NodeChange } from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import { Badge, Breadcrumbs, Button, ContextMenu, ContextMenuItem, SaveButton, SegmentedControl, Toggle, useToast } from '@kineticlogic/staresdk'
import { AnimatePresence, motion, useReducedMotion } from 'framer-motion'
import { CodeEditor } from '@kineticlogic/staresdk/code-editor'
import { FlowCanvas, type FlowCanvasEdge, type FlowCanvasNode, type FlowPaletteItem } from '@kineticlogic/staresdk/flow-canvas'
import {
  api,
  ApiError,
  type DecodedSamples,
  type PluginInfo,
  type PreviewResult,
  type ProbeResult,
  type SchemaOverview,
  type SourceProblem,
  type SourceRow,
  type SourceSpec,
} from '../../api/client'
import { useCan } from '../../auth/context'
import { InfoTip } from '../../components/InfoTip'
import { errorMessage } from '../../lib/format'
import { SOURCE_STATES_INFO, sourceState } from '../../lib/sourceState'
import { MAX_TRACED_SAMPLES, sampleTrail, TRACED_SAMPLES } from '../../lib/trace'
import { FeedPanel } from './flow/FeedPanel'
import { NodePanel } from './flow/NodePanel'
import { SamplesTray } from './flow/SamplesTray'
import { StepNode, type StepView } from './flow/StepNode'
import { stepIcon } from './flow/stepIcons'
import { tidy } from './flow/tidy'
import {
  canConnect,
  defaultConfig,
  flowOf,
  freePlace,
  guessTarget,
  joinDropped,
  INPUT,
  laidOut,
  mapField,
  nextNodeId,
  nodeSummary,
  nodeTitle,
  problemsByNode,
  STEPS,
  withFlow,
  withoutPositions,
  type Flow,
  type FlowNode,
  type StepType,
} from './flow/sourceFlow'

const REFRESH_MS = 5000
const PREVIEW_DELAY_MS = 600
const NODE_TYPES = { step: StepNode }

/** Whether parsed JSON has a source's shape, enough to show as steps (the server checks the rest). */
function isSpecShaped(v: unknown): v is SourceSpec {
  const o = (x: unknown): x is Record<string, unknown> => typeof x === 'object' && x !== null && !Array.isArray(x)
  if (!o(v) || typeof v.id !== 'string' || typeof v.name !== 'string' || !o(v.transport) || typeof v.transport.type !== 'string') return false
  const p = v.pipeline
  if (!o(p) || !o(p.codec) || typeof p.codec.type !== 'string' || !Array.isArray(p.nodes) || !Array.isArray(p.edges)) return false
  return (
    p.nodes.every((n) => o(n) && typeof n.id === 'string' && typeof n.type === 'string') &&
    p.edges.every((e) => o(e) && typeof e.source === 'string' && typeof e.target === 'string')
  )
}

/** The height of the element the ref is on, as it resizes: the JSON view fills the page with it. */
function useBoxHeight(): [(el: HTMLElement | null) => void, number] {
  const [height, setHeight] = useState(400)
  const observer = useRef<ResizeObserver | null>(null)
  const ref = useCallback((el: HTMLElement | null) => {
    observer.current?.disconnect()
    observer.current = null
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => {
      const h = Math.floor(el.getBoundingClientRect().height)
      if (h > 0) setHeight((old) => (old === h ? old : h))
    })
    ro.observe(el)
    observer.current = ro
  }, [])
  useEffect(() => () => observer.current?.disconnect(), [])
  return [ref, height]
}

/**
 * A dry run of the source as edited over its stored samples, re-run shortly after it stops changing
 * (moving a step does not count): what stops it running, each problem on its step, and its samples
 * step by step. A draft runs as far as it can. Nothing is saved or published.
 */
function useLiveCheck(spec: SourceSpec | null, id: string, nonce: number, trace: number, enabled: boolean) {
  const [preview, setPreview] = useState<PreviewResult | null>(null)
  const [problems, setProblems] = useState<SourceProblem[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [completed, setCompleted] = useState('')
  const seq = useRef(0)
  const key = spec ? JSON.stringify(withoutPositions(spec)) : ''
  const requestKey = `${id}:${nonce}:${trace}:${key}`
  useEffect(() => {
    if (!enabled || !key) return
    const n = ++seq.current
    const t = setTimeout(() => {
      api.preview(JSON.parse(key) as SourceSpec, id, [], trace).then(
        (r) => {
          if (n !== seq.current) return
          setPreview(r)
          setProblems(r.problems ?? [])
          setError(null)
          setCompleted(requestKey)
        },
        (e) => {
          if (n !== seq.current) return
          setPreview(null)
          const listed = e instanceof ApiError && e.problems.length > 0
          setProblems(listed ? e.problems : null)
          setError(listed ? null : errorMessage(e))
          setCompleted(requestKey)
        },
      )
    }, PREVIEW_DELAY_MS)
    return () => clearTimeout(t)
  }, [key, id, trace, enabled, requestKey])
  return { preview, problems, error, pending: enabled && !!key && completed !== requestKey }
}

/** The steps from the input to `id`. */
function pathTo(flow: Flow, id: string): string[] {
  const parent = new Map(flow.edges.map((e) => [e.target, e.source]))
  const out = [id]
  let at = id
  while (parent.has(at) && out.length <= flow.nodes.length) {
    at = parent.get(at)!
    out.unshift(at)
  }
  return out
}

/**
 * One source as a workspace of steps (#104): the input and the steps
 * the user places and connects, to the source's Output and NATS topics. A click selects a sample stage
 * and a double click opens settings. The feed's own settings open from the top row. Nothing
 * changes on the running source until Save.
 */
export default function SourceEditor({ id, onClose, onDeleted }: { id: string; onClose: () => void; onDeleted: () => void }) {
  const { confirm, toast } = useToast()
  const canAdmin = useCan('admin')
  const readOnly = !canAdmin
  const reduceMotion = useReducedMotion()
  const [row, setRow] = useState<SourceRow | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [spec, setSpec] = useState<SourceSpec | null>(null)
  const [baseline, setBaseline] = useState<string>('')
  const [selected, setSelected] = useState<string | null>(null)
  const [selectedEdges, setSelectedEdges] = useState<ReadonlySet<string>>(new Set())
  const [feedOpen, setFeedOpen] = useState(false)
  const [sampleStep, setSampleStep] = useState<string | null>(null)
  const [editingStep, setEditingStep] = useState<string | null>(null)
  const [stepMenu, setStepMenu] = useState<{ id: string; x: number; y: number } | null>(null)
  const [view, setView] = useState<'flow' | 'json'>('flow')
  const [jsonText, setJsonText] = useState('')
  const [jsonBox, jsonHeight] = useBoxHeight()
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [schema, setSchema] = useState<SchemaOverview | null>(null)
  const [plugins, setPlugins] = useState<PluginInfo[]>([])
  const [probe, setProbe] = useState<ProbeResult | null>(null)
  const [decoded, setDecoded] = useState<{ key: string; samples: DecodedSamples | null; error: string | null }>({ key: '', samples: null, error: null })
  const [capturing, setCapturing] = useState(false)
  const [nonce, setNonce] = useState(0)
  const [trayOpen, setTrayOpen] = useState(false)
  const [stepsOpen, setStepsOpen] = useState(false)
  // A traced sample followed through the flow (from 0).
  const [followed, setFollowed] = useState<number | null>(null)

  // A source from before flows has no places for its steps: they are laid out once, as loaded.
  const adopt = useCallback((r: SourceRow) => {
    const s = withFlow(r.spec, laidOut(flowOf(r.spec)))
    setRow(r)
    setSpec(s)
    setBaseline(JSON.stringify(s))
  }, [])

  // The row (its status, enabled, stored problems) refreshes; the spec being edited does not.
  useEffect(() => {
    let first = true
    const load = () =>
      api.source(id).then(
        (r) => {
          setLoadError(null)
          if (first) adopt(r)
          else setRow(r)
          first = false
        },
        (e) => setLoadError(errorMessage(e)),
      )
    load()
    const t = setInterval(load, REFRESH_MS)
    return () => clearInterval(t)
  }, [id, adopt])

  useEffect(() => {
    api.schema().then(setSchema, () => setSchema(null))
    api.plugins().then(setPlugins, () => setPlugins([]))
  }, [])

  const flow = useMemo<Flow>(() => (spec ? flowOf(spec) : { nodes: [], edges: [] }), [spec])
  const dirty = spec !== null && JSON.stringify(spec) !== baseline
  const current = selected && flow.nodes.some((n) => n.id === selected) ? selected : null
  const sampleNode = flow.nodes.find((n) => n.id === sampleStep)
  const editNode = flow.nodes.find((n) => n.id === editingStep)
  const traceCount = sampleNode?.type === 'filter' || editNode?.type === 'filter' ? MAX_TRACED_SAMPLES : TRACED_SAMPLES
  const live = useLiveCheck(spec, id, nonce, traceCount, canAdmin)
  const codecKey = spec ? JSON.stringify(spec.pipeline.codec) : ''
  const sampleKey = `${id}:${nonce}:${codecKey}`
  const decodedSamples = decoded.key === sampleKey ? decoded.samples : null
  const decodedError = decoded.key === sampleKey ? decoded.error : null
  useEffect(() => {
    if (!canAdmin || !codecKey) return
    let active = true
    api.decodeSamples(id, JSON.parse(codecKey) as SourceSpec['pipeline']['codec']).then(
      (samples) => { if (active) setDecoded({ key: sampleKey, samples, error: null }) },
      (e) => { if (active) setDecoded({ key: sampleKey, samples: null, error: errorMessage(e) }) },
    )
    return () => { active = false }
  }, [canAdmin, codecKey, id, sampleKey])
  // What stops it running: the live check's, else what was found when it was saved.
  const problems = useMemo(() => live.problems ?? row?.problems ?? [], [live.problems, row?.problems])
  const byNode = useMemo(() => problemsByNode(problems, flow), [problems, flow])

  const edit = (next: SourceSpec) => {
    setSpec(next)
    setSaved(false)
    setNotice(null)
  }
  const editFlow = (f: Flow) => {
    if (spec) edit(withFlow(spec, f))
  }

  /** The one thing to do next on a step's card: capture samples while there are none, use a suggested mapping on an empty Map. */
  const actionFor = (n: FlowNode): StepView['action'] => {
    if (readOnly) return undefined
    if (n.type === 'input' && live.preview && !(live.preview.steps?.input?.in ?? 0)) {
      return { label: capturing ? 'Capturing…' : 'Capture samples', icon: <TbDownload />, busy: capturing, onClick: () => void capture() }
    }
    const suggested = probe?.suggestion.mapping
    const rules = ((n.config as { mapping?: { rules?: unknown[] } } | undefined)?.mapping?.rules ?? []).length
    if (n.type === 'map' && suggested && suggested.rules.length > 0 && rules === 0) {
      return {
        label: 'Use suggested mapping',
        icon: <TbWand />,
        onClick: () => setConfig(n.id, { ...(n.config as object), mapping: { ...suggested, reject: undefined } }),
      }
    }
    return undefined
  }

  /** A field dragged from the samples onto a Map is mapped where it most likely goes. */
  const dropField = (n: FlowNode, path: string) => {
    if (!path) return
    setConfig(n.id, mapField(n.config, path, guessTarget(path, probe?.suggestion.proposals)))
    setSelected(n.id)
  }

  const openStepMenu = (nodeId: string, x: number, y: number) => {
    setStepMenu({ id: nodeId, x, y })
  }

  const editStep = (nodeId: string) => {
    setSelected(nodeId)
    setSelectedEdges(new Set())
    setEditingStep(nodeId)
    setFeedOpen(false)
    setStepsOpen(false)
  }

  const trail = useMemo(() => (followed === null ? null : sampleTrail(live.preview?.trace, followed)), [followed, live.preview])

  const nodes: FlowCanvasNode[] = useMemo(() => {
    if (!spec) return []
    return flow.nodes.map((n) => ({
      id: n.id,
      type: 'step',
      position: n.position ?? { x: 0, y: 0 },
      selected: n.id === current,
      deletable: n.type !== 'input',
      data: {
        type: n.type,
        icon: stepIcon(spec, n.type),
        title: nodeTitle(spec, n),
        summary: nodeSummary(spec, n),
        problems: byNode.byNode[n.id] ?? [],
        counts: live.preview?.steps?.[n.id],
        trail: trail ? (trail[n.id] ?? 'absent') : undefined,
        action: actionFor(n),
        onFieldDrop: n.type === 'map' && !readOnly ? (path) => dropField(n, path) : undefined,
        editing: n.id === editingStep,
        onEdit: () => editStep(n.id),
        onMenu: (x, y) => openStepMenu(n.id, x, y),
      } satisfies StepView,
    }))
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [spec, flow, current, byNode, live.preview, trail, probe, capturing, readOnly, editingStep])

  const edges: FlowCanvasEdge[] = useMemo(
    () =>
      flow.edges.map((e) => {
        const eid = `${e.source}->${e.target}`
        // What crossed it in the dry run: what its target took in.
        const crossed = live.preview?.steps?.[e.target]?.in
        // A followed sample's way through: from a step it passed to one it reached.
        const taken = trail ? trail[e.source]?.state === 'passed' && !!trail[e.target] : undefined
        return {
          id: eid,
          source: e.source,
          target: e.target,
          selected: selectedEdges.has(eid),
          label: crossed === undefined ? undefined : String(crossed),
          animated: taken,
          className: taken === undefined ? undefined : taken ? 'flow-edge-taken' : 'flow-edge-faded',
        }
      }),
    [flow, selectedEdges, live.preview, trail],
  )

  const transportType = spec?.transport.type
  const palette: FlowPaletteItem[] = useMemo(
    () =>
      (Object.keys(STEPS) as Exclude<StepType, 'input'>[]).map((t) => ({
        type: t,
        label: STEPS[t].label,
        group: STEPS[t].group,
        icon: stepIcon({ transport: { type: transportType ?? '' } } as SourceSpec, t),
      })),
    [transportType],
  )

  const updateSteps = (moved: Map<string, { x: number; y: number }>, removed: Set<string>) => {
    if (!moved.size && !removed.size) return
    const edges = flow.edges.filter((e) => !removed.has(e.source) && !removed.has(e.target))
    editFlow({
      nodes: flow.nodes
        .filter((n) => !removed.has(n.id))
        .map((n) => {
          const p = moved.get(n.id)
          return p ? { ...n, position: { x: Math.round(p.x), y: Math.round(p.y) } } : n
        }),
      edges,
    })
    if (removed.size) {
      setSelected((id) => id && removed.has(id) ? null : id)
      setEditingStep((id) => id && removed.has(id) ? null : id)
      setSampleStep((id) => id && removed.has(id) ? INPUT : id)
      const validEdges = new Set(edges.map((edge) => `${edge.source}->${edge.target}`))
      setSelectedEdges((ids) => new Set([...ids].filter((id) => validEdges.has(id))))
    }
  }

  // React Flow also selects a step when a drag starts. Selection alone does not open a view.
  const onNodesChange = (changes: NodeChange[]) => {
    const picks = changes.flatMap((c) => (c.type === 'select' ? [c] : []))
    if (picks.length) {
      const on = picks.find((c) => c.selected)
      if (on) setSelected(on.id)
      else if (picks.some((c) => c.id === selected)) setSelected(null)
    }
    const moved = new Map(changes.flatMap((c) => (c.type === 'position' && c.position ? [[c.id, c.position] as const] : [])))
    const removed = new Set(changes.flatMap((c) => (c.type === 'remove' && c.id !== INPUT ? [c.id] : [])))
    updateSteps(moved, removed)
  }

  const selectRouteStage = (edgeId: string) => {
    const route = flow.edges.find((edge) => `${edge.source}->${edge.target}` === edgeId)
    if (route) setSampleStep(route.target)
  }

  const onEdgesChange = (changes: EdgeChange[]) => {
    const picks = changes.flatMap((c) => (c.type === 'select' ? [c] : []))
    if (picks.length) {
      setSelectedEdges((prev) => {
        const next = new Set(prev)
        for (const c of picks) {
          if (c.selected) next.add(c.id)
          else next.delete(c.id)
        }
        return next
      })
      const added = picks.findLast((pick) => pick.selected)
      if (added) selectRouteStage(added.id)
    }
    const removed = new Set(changes.flatMap((c) => (c.type === 'remove' ? [c.id] : [])))
    if (removed.size) editFlow({ ...flow, edges: flow.edges.filter((e) => !removed.has(`${e.source}->${e.target}`)) })
  }

  const isValidConnection = (c: Connection | Edge) => !!c.source && !!c.target && canConnect(flow, c.source, c.target)

  const onConnect = (c: Connection) => {
    if (!c.source || !c.target || !canConnect(flow, c.source, c.target)) return
    editFlow({ ...flow, edges: [...flow.edges, { source: c.source, target: c.target }] })
  }

  const onDropNode = (type: string, position: { x: number; y: number }) => {
    if (!spec || readOnly || !(type in STEPS)) return
    const t = type as Exclude<StepType, 'input'>
    if (t === 'output' && flow.nodes.some((n) => n.type === 'output')) {
      setNotice('A source has one Output.')
      return
    }
    const node: FlowNode = {
      id: nextNodeId(
        t,
        flow.nodes.map((n) => n.id),
      ),
      type: t,
      position: freePlace(flow, position),
    }
    const config = defaultConfig(t)
    if (config !== undefined) node.config = config
    editFlow(joinDropped({ ...flow, nodes: [...flow.nodes, node] }, node.id))
    setSelected(node.id)
    setStepsOpen(false)
  }

  const setConfig = (nodeId: string, config: unknown) => editFlow({ ...flow, nodes: flow.nodes.map((n) => (n.id === nodeId ? { ...n, config } : n)) })

  const save = async () => {
    if (!spec) return
    setSaving(true)
    setNotice(null)
    try {
      adopt(await api.saveSource(spec))
      setSaved(true)
      setNonce((n) => n + 1)
    } catch (e) {
      setNotice(errorMessage(e))
    } finally {
      setSaving(false)
    }
  }

  const toggle = async (on: boolean) => {
    if (on && dirty) {
      setNotice('Save the source before turning it on.')
      return
    }
    try {
      setRow(await api.setEnabled(id, on))
      setNotice(null)
    } catch (e) {
      setNotice(errorMessage(e))
    }
  }

  const remove = async () => {
    const ok = await confirm(`Delete source ${id}? Its configuration history stays in the decision log.`, { title: 'Delete source', confirmLabel: 'Delete' })
    if (!ok) return
    try {
      await api.deleteSource(id)
      onDeleted()
    } catch (e) {
      toast({ variant: 'error', message: errorMessage(e) })
    }
  }

  const leave = async () => {
    if (dirty && !(await confirm('Leave without saving? The changes to this source are lost.', { title: 'Unsaved source', confirmLabel: 'Discard' }))) return
    onClose()
  }

  /** Take a few frames from the feed now, keep them as the source's samples, and offer what they suggest. */
  const capture = async () => {
    if (!spec) return
    setCapturing(true)
    try {
      const r = await api.probe({ transport: spec.transport, codec: spec.pipeline.codec, max_frames: 20, max_secs: 15, save_as: id })
      setProbe(r)
      if (!r.frames) toast({ variant: 'warning', message: `No frames captured: ${r.link_error ?? r.hint ?? 'none arrived in time.'}` })
      setNonce((n) => n + 1)
    } catch (e) {
      toast({ variant: 'error', title: 'Capture failed', message: errorMessage(e) })
    } finally {
      setCapturing(false)
    }
  }

  const switchView = (v: 'flow' | 'json') => {
    if (v === 'json' && spec) setJsonText(JSON.stringify(spec, null, 2))
    setStepsOpen(false)
    setView(v)
  }

  if (!spec || !row) {
    return (
      <div className="panels">
        <div className="panel-body">{loadError ? <div className="error-text">{loadError}</div> : <span className="muted">LOADING…</span>}</div>
      </div>
    )
  }

  const state = sourceState(row)
  const general = [...(notice ? [notice] : []), ...byNode.general]
  const output = flow.nodes.find((n) => n.type === 'output')
  const tracked = !!output && pathTo(flow, output.id).some((s) => flow.nodes.find((n) => n.id === s)?.type === 'tracker')
  const showStep = view === 'flow' ? editNode : undefined
  const showSamples = view === 'flow' && trayOpen
  const menuNode = stepMenu && flow.nodes.find((n) => n.id === stepMenu.id)

  return (
    <div className="source-editor" onKeyDown={(event) => {
      if (event.key !== 'Escape' || event.defaultPrevented || stepMenu) return
      if (stepsOpen) setStepsOpen(false)
      else if (showStep || feedOpen) {
        setEditingStep(null)
        setFeedOpen(false)
      } else if (showSamples) setTrayOpen(false)
    }}>
      <div className="source-editor-head">
        <Breadcrumbs
          items={[{ label: 'Sources', href: '#sources' }, { label: spec.name || id }]}
          onNavigate={() => {
            void leave()
          }}
        />
        <Badge color={state.color} size="sm" uppercase>
          {state.label}
        </Badge>
        <InfoTip label="Source state">{SOURCE_STATES_INFO}</InfoTip>
        {general.length > 0 && (
          <span className="error-text source-editor-general" role="alert" title={general.join('\n')}>
            {general.join(' · ')}
          </span>
        )}
        <span className="spacer" />
        <SegmentedControl
          aria-label="Source view"
          size="sm"
          value={view}
          onChange={switchView}
          options={[
            { value: 'flow', label: 'Flow' },
            { value: 'json', label: 'JSON' },
          ]}
        />
        {view === 'flow' && (
          <>
            {!readOnly && (
              <Button size="sm" variant="secondary" icon={<TbList />} aria-expanded={stepsOpen} active={stepsOpen} onClick={() => setStepsOpen((open) => !open)}>
                Steps
              </Button>
            )}
            <Button size="sm" variant="secondary" icon={<TbTable />} aria-expanded={showSamples} active={showSamples} onClick={() => {
              if (!trayOpen && !sampleNode) setSampleStep(current ?? INPUT)
              setTrayOpen(!trayOpen)
            }}>
              Samples
            </Button>
            <Button size="sm" variant="secondary" icon={<TbLayoutDistributeVertical />} disabled={readOnly} onClick={() => editFlow(tidy(flow))}>
              Tidy
            </Button>
          </>
        )}
        <Button
          size="sm"
          variant={feedOpen ? 'primary' : 'secondary'}
          icon={<TbSettings />}
          onClick={() => {
            setFeedOpen((o) => !o)
            setEditingStep(null)
            setStepsOpen(false)
          }}
        >
          Feed
        </Button>
        <label className="row" style={{ gap: 6 }}>
          <span className="muted">Enabled</span>
          <Toggle size="sm" value={row.enabled} disabled={readOnly} onChange={toggle} aria-label="Enabled" />
        </label>
        {canAdmin && <SaveButton size="sm" dirty={dirty} saving={saving} saved={saved} onSave={save} />}
        <Button size="xs" variant="ghost" icon={<TbTrash />} disabled={readOnly} aria-label="Delete source" title="Delete source" onClick={remove} />
      </div>
      <div className={showStep?.type === 'map' && showSamples ? 'source-workspace source-workspace--map-compare' : 'source-workspace'}>
        {view === 'flow' ? (
          <div
            className="source-workspace-canvas"
            onClick={(event) => {
              const edge = (event.target as Element).closest('.react-flow__edge[data-id]')?.getAttribute('data-id')
              if (edge) selectRouteStage(edge)
            }}
            onKeyDown={(event) => {
              const target = event.target as HTMLElement
              const nodeId = target.classList.contains('react-flow__node') ? target.getAttribute('data-id') : null
              if (!nodeId) return
              if (event.key === 'Enter' || event.key === 'F2') {
                event.preventDefault()
                if (event.key === 'F2') editStep(nodeId)
                else setSampleStep(nodeId)
              } else if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) {
                event.preventDefault()
                const rect = target.getBoundingClientRect()
                openStepMenu(nodeId, rect.left + rect.width / 2, rect.top + rect.height / 2)
              }
            }}
          >
            <FlowCanvas
              aria-label={`Steps of ${spec.name || id}`}
              nodes={nodes}
              edges={edges}
              nodeTypes={NODE_TYPES}
              onNodesChange={onNodesChange}
              onNodeClick={setSampleStep}
              onEdgesChange={onEdgesChange}
              onConnect={onConnect}
              isValidConnection={isValidConnection}
              palette={stepsOpen ? palette : undefined}
              onDropNode={onDropNode}
              readOnly={readOnly}
              height="100%"
              style={{ border: 0, borderRadius: 0 }}
            />
          </div>
        ) : (
          <div className="source-workspace-canvas source-editor-json" ref={jsonBox}>
            <CodeEditor
              aria-label="Source settings as JSON"
              minHeight={jsonHeight}
              maxHeight={jsonHeight}
              value={jsonText}
              readOnly={readOnly}
              onChange={(t) => {
                setJsonText(t)
                try {
                  const next = JSON.parse(t) as unknown
                  if (isSpecShaped(next) && next.id === id) edit(next)
                } catch {
                  // Keep the last valid settings; the editor marks the parse error.
                }
              }}
            />
          </div>
        )}
        {(showStep || feedOpen) && (
          <aside className="source-inspector" aria-label={showStep ? `Step ${showStep.id}` : 'Feed settings'}>
            {showStep ? (
              <NodePanel
                key={showStep.id}
                spec={spec}
                node={showStep}
                path={pathTo(flow, showStep.id)}
                problems={byNode.byNode[showStep.id] ?? []}
                schema={schema}
                plugins={plugins}
                preview={live.preview}
                decodedSamples={decodedSamples}
                decodedError={decodedError}
                previewPending={live.pending}
                probe={probe}
                capturing={capturing}
                readOnly={readOnly}
                onSpec={edit}
                onConfig={(c) => setConfig(showStep.id, c)}
                onCapture={() => void capture()}
                onClose={() => setEditingStep(null)}
              />
            ) : (
              <FeedPanel spec={spec} row={row} tracked={tracked} problems={byNode.general} readOnly={readOnly} onChange={edit} onClose={() => setFeedOpen(false)} />
            )}
          </aside>
        )}
        <AnimatePresence initial={false}>
          {showSamples && (
            <motion.div
              className="source-samples"
              initial={{ y: '100%' }}
              animate={{ y: 0, pointerEvents: 'auto' }}
              exit={{ y: '100%', pointerEvents: 'none' }}
              transition={{ duration: reduceMotion ? 0 : 0.25, ease: 'easeInOut' }}
            >
              <SamplesTray
                node={sampleNode ?? null}
                title={sampleNode ? nodeTitle(spec, sampleNode) : ''}
                path={sampleNode ? pathTo(flow, sampleNode.id) : []}
                preview={live.preview}
                previewPending={live.pending}
                decodedSamples={decodedSamples}
                decodedError={decodedError}
                error={live.error}
                open
                onOpenChange={setTrayOpen}
                followed={followed}
                onFollow={setFollowed}
              />
            </motion.div>
          )}
        </AnimatePresence>
        {stepMenu && menuNode && (
          <ContextMenu x={stepMenu.x} y={stepMenu.y} width={160} estimatedHeight={82} size="sm" ariaLabel="Step actions" onClose={() => setStepMenu(null)}>
            <ContextMenuItem onClick={() => { setStepMenu(null); editStep(menuNode.id) }}>
              {readOnly ? 'View settings' : 'Edit'}
            </ContextMenuItem>
            <ContextMenuItem danger disabled={readOnly || menuNode.id === INPUT} onClick={() => {
              if (readOnly || menuNode.id === INPUT) return
              setStepMenu(null)
              updateSteps(new Map(), new Set([menuNode.id]))
            }}>
              Delete
            </ContextMenuItem>
          </ContextMenu>
        )}
      </div>
    </div>
  )
}
