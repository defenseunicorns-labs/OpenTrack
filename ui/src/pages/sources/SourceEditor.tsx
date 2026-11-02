import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { TbSettings, TbTrash } from 'react-icons/tb'
import type { Connection, Edge, EdgeChange, NodeChange } from '@xyflow/react'
import '@xyflow/react/dist/style.css'
import { Badge, Breadcrumbs, Button, SaveButton, SegmentedControl, SideNav, Toggle, useToast } from '@kineticlogic/staresdk'
import { CodeEditor } from '@kineticlogic/staresdk/code-editor'
import { FlowCanvas, type FlowCanvasEdge, type FlowCanvasNode, type FlowPaletteItem } from '@kineticlogic/staresdk/flow-canvas'
import {
  api,
  ApiError,
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
import {
  canConnect,
  defaultConfig,
  flowOf,
  freePlace,
  INPUT,
  laidOut,
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
  const seq = useRef(0)
  const key = spec ? JSON.stringify(withoutPositions(spec)) : ''
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
        },
        (e) => {
          if (n !== seq.current) return
          setPreview(null)
          const listed = e instanceof ApiError && e.problems.length > 0
          setProblems(listed ? e.problems : null)
          setError(listed ? null : errorMessage(e))
        },
      )
    }, PREVIEW_DELAY_MS)
    return () => clearTimeout(t)
  }, [key, id, nonce, trace, enabled])
  return { preview, problems, error }
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
 * One source as a workspace of steps (#104): the step list on the left; the input, and the steps
 * the user places and connects, to the source's Output and NATS topics. Clicking a step opens its
 * settings and samples; the feed's own settings, status and history open from the top row. Nothing
 * changes on the running source until Save.
 */
export default function SourceEditor({ id, onClose, onDeleted }: { id: string; onClose: () => void; onDeleted: () => void }) {
  const { confirm, toast } = useToast()
  const canAdmin = useCan('admin')
  const readOnly = !canAdmin
  const [row, setRow] = useState<SourceRow | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [spec, setSpec] = useState<SourceSpec | null>(null)
  const [baseline, setBaseline] = useState<string>('')
  const [selected, setSelected] = useState<string | null>(null)
  const [selectedEdges, setSelectedEdges] = useState<ReadonlySet<string>>(new Set())
  const [feedOpen, setFeedOpen] = useState(false)
  // A step's settings can be closed and the step stay selected, its samples in the tray.
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [view, setView] = useState<'flow' | 'json'>('flow')
  const [jsonText, setJsonText] = useState('')
  const [jsonBox, jsonHeight] = useBoxHeight()
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)
  const [notice, setNotice] = useState<string | null>(null)
  const [schema, setSchema] = useState<SchemaOverview | null>(null)
  const [plugins, setPlugins] = useState<PluginInfo[]>([])
  const [probe, setProbe] = useState<ProbeResult | null>(null)
  const [capturing, setCapturing] = useState(false)
  const [nonce, setNonce] = useState(0)
  const [trayOpen, setTrayOpen] = useState(true)
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
  const currentType = flow.nodes.find((n) => n.id === current)?.type
  const live = useLiveCheck(spec, id, nonce, currentType === 'filter' ? MAX_TRACED_SAMPLES : TRACED_SAMPLES, canAdmin)
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
      } satisfies StepView,
    }))
  }, [spec, flow, current, byNode, live.preview, trail])

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

  // Steps move where they are dragged; clicking one opens it; deleting one takes its connections.
  const onNodesChange = (changes: NodeChange[]) => {
    const picks = changes.flatMap((c) => (c.type === 'select' ? [c] : []))
    if (picks.length) {
      const on = picks.find((c) => c.selected)
      if (on) {
        setSelected(on.id)
        setSettingsOpen(true)
        setFeedOpen(false)
      } else if (picks.some((c) => c.id === selected)) setSelected(null)
    }
    const moved = new Map(changes.flatMap((c) => (c.type === 'position' && c.position ? [[c.id, c.position] as const] : [])))
    const removed = new Set(changes.flatMap((c) => (c.type === 'remove' && c.id !== INPUT ? [c.id] : [])))
    if (!moved.size && !removed.size) return
    editFlow({
      nodes: flow.nodes
        .filter((n) => !removed.has(n.id))
        .map((n) => {
          const p = moved.get(n.id)
          return p ? { ...n, position: { x: Math.round(p.x), y: Math.round(p.y) } } : n
        }),
      edges: flow.edges.filter((e) => !removed.has(e.source) && !removed.has(e.target)),
    })
    if (selected && removed.has(selected)) setSelected(null)
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
    editFlow({ ...flow, nodes: [...flow.nodes, node] })
    setSelected(node.id)
    setSettingsOpen(true)
    setFeedOpen(false)
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
  const node = flow.nodes.find((n) => n.id === current)
  const output = flow.nodes.find((n) => n.type === 'output')
  const tracked = !!output && pathTo(flow, output.id).some((s) => flow.nodes.find((n) => n.id === s)?.type === 'tracker')

  return (
    <div className="source-editor">
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
        <Button
          size="sm"
          variant={feedOpen ? 'primary' : 'secondary'}
          icon={<TbSettings />}
          onClick={() => {
            setFeedOpen((o) => !o)
            setSettingsOpen(false)
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
      {view === 'flow' ? (
        <div className="source-workspace">
          <div className="source-workspace-canvas">
            <FlowCanvas
              aria-label={`Steps of ${spec.name || id}`}
              nodes={nodes}
              edges={edges}
              nodeTypes={NODE_TYPES}
              onNodesChange={onNodesChange}
              onEdgesChange={onEdgesChange}
              onConnect={onConnect}
              isValidConnection={isValidConnection}
              palette={palette}
              onDropNode={onDropNode}
              readOnly={readOnly}
              height="100%"
            />
          </div>
          <SamplesTray
            node={node ?? null}
            title={node ? nodeTitle(spec, node) : ''}
            path={node ? pathTo(flow, node.id) : []}
            preview={live.preview}
            error={live.error}
            open={trayOpen}
            onOpenChange={setTrayOpen}
            followed={followed}
            onFollow={setFollowed}
          />
          {/* A step's settings (or the feed's) open over the right of the page, resizable. */}
          <SideNav
            open={(!!node && settingsOpen) || feedOpen}
            ariaLabel={node ? `Step ${node.id}` : 'Feed settings'}
            width={640}
            minWidth={420}
            maxWidth={1200}
            storageKey="ot.sourceStep.width"
            dockRegister={false}
          >
            <div
              className="drawer"
              onKeyDown={(e) => {
                if (e.key === 'Escape') {
                  setSettingsOpen(false)
                  setFeedOpen(false)
                }
              }}
            >
              {node && settingsOpen ? (
                <NodePanel
                  key={node.id}
                  spec={spec}
                  node={node}
                  path={pathTo(flow, node.id)}
                  problems={byNode.byNode[node.id] ?? []}
                  schema={schema}
                  plugins={plugins}
                  preview={live.preview}
                  probe={probe}
                  capturing={capturing}
                  readOnly={readOnly}
                  onSpec={edit}
                  onConfig={(c) => setConfig(node.id, c)}
                  onCapture={() => void capture()}
                  onClose={() => setSettingsOpen(false)}
                />
              ) : feedOpen ? (
                <FeedPanel spec={spec} row={row} tracked={tracked} problems={byNode.general} readOnly={readOnly} onChange={edit} onClose={() => setFeedOpen(false)} />
              ) : null}
            </div>
          </SideNav>
        </div>
      ) : (
        <div className="source-editor-json" ref={jsonBox}>
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
    </div>
  )
}
