import { lazy, Suspense, useEffect, useMemo, useState } from 'react'
import { TbDownload, TbPlus, TbTrash } from 'react-icons/tb'
import { Button, Modal, SaveButton, useToast } from '@kineticlogic/staresdk'
import { api, type SchemaOverview, type SourceSpec } from '../../../api/client'
import { InfoTip } from '../../../components/InfoTip'
import { errorMessage } from '../../../lib/format'
import { pipelineStages, DEFAULT_LINKS } from '../../../lib/pipeline'
import { filterInput, MAX_TRACED_SAMPLES, TRACED_SAMPLES } from '../../../lib/trace'
import { sampleFields } from '../../../lib/conditions'
import { usePreview } from '../../../lib/usePreview'
import { MapEditor } from './MapEditor'
import { StageSamples } from './StageSamples'
import { AffiliationForm, DecodeForm, FilterForm, JoinForm, PublishForm, RegistryForm, RejectForm, ThrottleForm, TrackerForm, SecurityForm, EmitterMotionForm } from './StageForms'

const PipelineFlow = lazy(() => import('../PipelineFlow'))

type Obj = Record<string, unknown>
type Pipeline = SourceSpec['pipeline'] & Obj

/** Optional stages the user can add, with what a new one starts as. */
const OPTIONAL: { id: string; label: string; add: (p: Pipeline) => Pipeline; remove: (p: Pipeline) => Pipeline }[] = [
  {
    id: 'reject',
    label: 'Reject',
    add: (p) => ({ ...p, mapping: { ...p.mapping, reject: [{ reason: 'no_position', when: { path: 'lat', exists: false } }] } }),
    remove: (p) => ({ ...p, mapping: { ...p.mapping, reject: undefined } }),
  },
  {
    id: 'registry',
    label: 'Entity links',
    add: (p) => ({ ...p, registry: { links: DEFAULT_LINKS } }),
    remove: (p) => ({ ...p, registry: undefined }),
  },
  {
    id: 'affiliation',
    label: 'Affiliation',
    add: (p) => ({ ...p, affiliation: { country: 'platform.flag', friendly: [], hostile: [], neutral: [] } }),
    remove: (p) => ({ ...p, affiliation: undefined }),
  },
  {
    id: 'filter',
    label: 'Filter',
    add: (p) => ({ ...p, filter: {} }),
    remove: (p) => ({ ...p, filter: undefined }),
  },
  {
    id: 'tracker',
    label: 'Tracker',
    add: (p) => ({ ...p, tracker: { algorithm: 'gnn' } }),
    remove: (p) => ({ ...p, tracker: undefined }),
  },
  {
    id: 'throttle',
    label: 'Throttle',
    add: (p) => ({ ...p, throttle: { min_interval_secs: 5, min_move_m: 0, heartbeat_secs: 600 } }),
    remove: (p) => ({ ...p, throttle: undefined }),
  },
]

/** What each stage does, for the ⓘ beside its heading. */
const STAGE_HELP: Record<string, string> = {
  transport: 'Where frames come from. Set on the source’s Transport tab.',
  decode: 'Turns each frame into records.',
  reject: 'Drops raw records before mapping, each rule with a reason counted in the source’s metrics.',
  map: 'Turns records into observations: the track key, identifiers and OpenTrack fields each rule takes from the record.',
  join: 'Keeps the identity fields that static rules report per track key, and fills them into later observations with the same key.',
  registry: 'Finds each track’s entity by its identifiers, grades the match by broadcast name, and at a corroborated grade passes fields between entity and track.',
  affiliation: 'Sets affiliation from a country code, by the country lists.',
  filter: 'Keeps or drops observations after mapping and the entity stage; dropped ones are counted as filtered.',
  tracker: 'Forms tracks from detections (plots) before they leave the source.',
  throttle: 'Limits how often each source track is passed on, by time and distance moved.',
  publish: 'What the source’s observations are, which decides how correlation takes them, and the label they carry.',
}

const present = (p: Pipeline, id: string) =>
  id === 'reject' ? ((p.mapping.reject as unknown[]) ?? []).length > 0 : id === 'registry' ? p.registry !== undefined : p[id] !== undefined

/** Drop keys set to undefined, so the saved spec only carries what is configured. */
const clean = (p: Pipeline): Pipeline => JSON.parse(JSON.stringify(p))

/**
 * Design a source's pipeline stage by stage, with a live dry run over the stored samples. Saving
 * stores a new revision of the source (the worker restarts with it); nothing changes until then.
 */
export function PipelineDesigner({
  initial,
  sourceId,
  onClose,
  onSaved,
}: {
  initial: SourceSpec
  sourceId: string
  onClose: () => void
  onSaved: (spec: SourceSpec) => void
}) {
  const { confirm } = useToast()
  const [spec, setSpec] = useState<SourceSpec>(initial)
  const [stageId, setStageId] = useState('map')
  const [schema, setSchema] = useState<SchemaOverview | null>(null)
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [nonce, setNonce] = useState(0)
  const [capturing, setCapturing] = useState(false)
  // The filter builder offers the fields of every traced sample, so it asks for as many as there are.
  const { preview, error: previewError, running } = usePreview(spec, sourceId, nonce, stageId === 'filter' ? MAX_TRACED_SAMPLES : TRACED_SAMPLES)

  useEffect(() => {
    api.schema().then(setSchema, (e) => setError(errorMessage(e)))
  }, [])

  const p = spec.pipeline as Pipeline
  const setPipeline = (next: Pipeline) => setSpec({ ...spec, pipeline: clean(next) })
  const stages = useMemo(() => pipelineStages(spec), [spec])
  const stage = stages.find((s) => s.id === stageId) ?? stages.find((s) => s.id === 'map')!
  const optional = OPTIONAL.find((o) => o.id === stage.id)
  const dirty = JSON.stringify(spec) !== JSON.stringify(initial)

  /** Take a few frames from the feed now and keep them as this source's samples. */
  const capture = async () => {
    setCapturing(true)
    setError(null)
    try {
      const r = await api.probe({ transport: spec.transport, codec: spec.pipeline.codec, max_frames: 20, max_secs: 15, save_as: sourceId })
      if (!r.frames && (r.link_error || r.hint)) setError(`No frames captured: ${r.link_error ?? r.hint}`)
      setNonce((n) => n + 1)
    } catch (e) {
      setError(errorMessage(e))
    } finally {
      setCapturing(false)
    }
  }

  const save = async () => {
    setSaving(true)
    setError(null)
    try {
      await api.saveSource(spec)
      setSaved(true)
      setTimeout(() => setSaved(false), 1500)
      onSaved(spec)
    } catch (e) {
      setError(errorMessage(e))
    } finally {
      setSaving(false)
    }
  }

  const close = async () => {
    if (dirty && !(await confirm('Close without saving? The pipeline changes are lost.', { title: 'Unsaved pipeline', confirmLabel: 'Discard' }))) return
    onClose()
  }

  let editor: React.ReactNode = null
  switch (stage.id) {
    case 'transport':
      editor = <span className="muted">The transport is set on the source's Transport tab.</span>
      break
    case 'decode':
      editor = <DecodeForm value={p.codec as Obj} onChange={(codec) => setPipeline({ ...p, codec: codec as Pipeline['codec'] })} />
      break
    case 'reject':
      editor = <RejectForm rules={(p.mapping.reject as Obj[]) ?? []} onChange={(reject) => setPipeline({ ...p, mapping: { ...p.mapping, reject: reject.length ? reject : undefined } })} />
      break
    case 'map':
      editor = schema ? (
        <MapEditor mapping={p.mapping as Obj} schema={schema} onChange={(mapping) => setPipeline({ ...p, mapping: mapping as Pipeline['mapping'] })} />
      ) : (
        <span className="muted">LOADING…</span>
      )
      break
    case 'join':
      editor = <JoinForm value={(p.static_join as Obj) ?? {}} onChange={(static_join) => setPipeline({ ...p, static_join })} />
      break
    case 'registry':
      editor = p.registry ? (
        <RegistryForm value={p.registry as Obj} onChange={(registry) => setPipeline({ ...p, registry })} />
      ) : (
        <span className="muted">
          Runs with defaults: resolves identifiers, and at a corroborated match the entity&apos;s name, class name, domain, affiliation, track type
          and symbol populate the track. Configure it to choose the links.
        </span>
      )
      break
    case 'affiliation':
      editor = <AffiliationForm value={p.affiliation as Obj} onChange={(affiliation) => setPipeline({ ...p, affiliation })} />
      break
    case 'filter':
      {
        const input = filterInput(preview?.trace)
        const fields = sampleFields([...input.observations, ...(preview?.observations ?? [])])
        const key = (o: unknown) => String((o as { source_track_key?: unknown }).source_track_key ?? '')
        const examples = [...new Set(input.dropped.map(key).filter(Boolean))].slice(0, 3)
        const effect = preview ? { dropped: preview.counts?.filtered ?? 0, examples } : null
        editor = <FilterForm value={(p.filter as Obj) ?? {}} onChange={(filter) => setPipeline({ ...p, filter })} fields={fields} effect={effect} />
      }
      break
    case 'tracker':
      editor = <TrackerForm value={p.tracker as Obj} onChange={(tracker) => setPipeline({ ...p, tracker })} />
      break
    case 'throttle':
      editor = <ThrottleForm value={p.throttle as Obj} onChange={(throttle) => setPipeline({ ...p, throttle })} />
      break
    case 'publish':
      editor = (
        <PublishForm
          reports={spec.reports}
          tracker={p.tracker !== undefined}
          publishAlone={spec.publish_alone}
          confirmAfter={spec.confirm_after}
          onChange={(reports) => setSpec({ ...spec, reports })}
          onPublishAlone={(publish_alone) => setSpec({ ...spec, publish_alone })}
          onConfirmAfter={(confirm_after) => setSpec({ ...spec, confirm_after })}
        />
      )
      editor = (
        <div className="stack">
          {editor}
          <SecurityForm value={spec.security} onChange={(security) => setSpec({ ...spec, security })} />
          <EmitterMotionForm value={spec.emitter_motion} onChange={(emitter_motion) => setSpec({ ...spec, emitter_motion })} />
        </div>
      )
      break
  }

  return (
    <Modal
      title={`Pipeline · ${spec.name}`}
      width="94vw"
      onClose={close}
      titleActions={<SaveButton size="sm" dirty={dirty} saving={saving} saved={saved} onSave={save} />}
    >
      <div className="designer">
        <div className="stack">
          <Suspense fallback={<span className="muted">LOADING…</span>}>
            <PipelineFlow stages={stages} selected={stage.id} onSelect={setStageId} />
          </Suspense>
          <div className="counts">
            {OPTIONAL.some((o) => !present(p, o.id)) && (
              <InfoTip label="Add a stage">
                Add an optional stage. Reject: drop raw records by condition, with a reason. Entity links: choose how tracks find their entity and which
                fields flow each way (without it, defaults apply). Affiliation: set affiliation from a country code. Filter: keep or drop observations by
                condition. Tracker: form tracks from detections. Throttle: pass each track on less often.
              </InfoTip>
            )}
            {OPTIONAL.filter((o) => !present(p, o.id)).map((o) => (
              <Button
                key={o.id}
                size="sm"
                variant="secondary"
                icon={<TbPlus />}
                onClick={() => {
                  setPipeline(o.add(p))
                  setStageId(o.id)
                }}
              >
                {o.label}
              </Button>
            ))}
          </div>
        </div>
        <div className="stack" style={{ minWidth: 0 }}>
          <div className="value-row">
            <h3 className="subhead">
              {stage.title}
              {STAGE_HELP[stage.id] && <InfoTip label={stage.title}>{STAGE_HELP[stage.id]}</InfoTip>}
            </h3>
            <span className="spacer" />
            {optional && present(p, optional.id) && (
              <Button
                size="sm"
                variant="ghost"
                icon={<TbTrash />}
                onClick={() => {
                  setPipeline(optional.remove(p))
                  setStageId('map')
                }}
              >
                {optional.id === 'registry' ? 'Use defaults' : 'Remove stage'}
              </Button>
            )}
            {optional && present(p, optional.id) && (
              <InfoTip label={optional.id === 'registry' ? 'Use defaults' : 'Remove stage'}>
                {optional.id === 'registry'
                  ? 'Drop these settings: the entity stage still runs, with the default links (the entity’s OTH-GOLD minimum populates the track) at grades exact, hull, name and generic.'
                  : 'Take this stage out of the pipeline. Nothing changes on the running source until you save.'}
              </InfoTip>
            )}
          </div>
          {error && <div className="error-text">{error}</div>}
          {editor}
        </div>
        <div className="stack designer-preview">
          <div className="value-row">
            <h3 className="subhead">
              Live preview
              <InfoTip label="Live preview">
                A dry run of the pipeline as edited (not yet saved) over this source&apos;s stored samples, updated as you change it. It shows the
                first {TRACED_SAMPLES} stored samples (decoded records, in order) as they are after the stage selected on the left: at Transport
                the frames they came from, then each record, its observation, and at Publish the message as it would be published. A sample
                dropped or held on the way says where and why. The counts cover everything stored. Nothing is published.
              </InfoTip>
            </h3>
            <span className="muted">{running ? 'running…' : `stored samples of ${sourceId}`}</span>
            <span className="spacer" />
            <Button size="sm" variant="secondary" icon={<TbDownload />} disabled={capturing} onClick={capture}>
              {capturing ? 'Capturing…' : 'Capture samples'}
            </Button>
            <InfoTip label="Capture samples">
              Connect with the transport as edited and take up to 20 frames or 15 s of the live feed, whichever comes first. They replace the
              source&apos;s stored samples.
            </InfoTip>
          </div>

          {previewError && <div className="error-text">{previewError}</div>}
          {preview ? <StageSamples result={preview} stageId={stage.id} /> : !previewError && <span className="muted">Running the first preview…</span>}
        </div>
      </div>
    </Modal>
  )
}
