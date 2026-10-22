import { TbDownload, TbWand, TbX } from 'react-icons/tb'
import { Button, Input, Label, Select } from '@kineticlogic/staresdk'
import type { PluginInfo, PreviewResult, ProbeResult, SchemaOverview, SourceSpec } from '../../../api/client'
import { InfoTip } from '../../../components/InfoTip'
import { sampleFields } from '../../../lib/conditions'
import { filterInput } from '../../../lib/trace'
import { MapEditor } from '../designer/MapEditor'
import { StageSamples } from '../designer/StageSamples'
import { AffiliationForm, FilterForm, JoinForm, RegistryForm, RejectForm, ThrottleForm, TrackerForm } from '../designer/StageForms'
import { SenderAuth } from '../SenderAuth'
import { TransportForm } from '../TransportForm'
import { codecChoice, codecChoices, KIND_WORD, nodeTitle, TRANSPORTS, withCodec, withTransport, type FlowNode, type StepType } from './sourceFlow'

type Obj = Record<string, unknown>

/** What each step does, for the ⓘ beside its heading. */
const STEP_HELP: Record<StepType, string> = {
  input: 'Where frames come from, and how each becomes records: the transport, how it connects or listens, how a stream is cut into frames, and the codec.',
  reject: 'Drops records before mapping, each rule with a reason counted in the source’s metrics.',
  map: 'Turns records into observations: the track key, identifiers and OpenTrack fields each rule takes from the record. Identity fields from static rules fill the same track’s later reports.',
  filter: 'Keeps or drops observations; dropped ones are counted as filtered.',
  registry: 'Finds each track’s entity by its identifiers, grades the match by broadcast name, and at a corroborated grade passes fields between entity and track. Without one before the Output, the defaults run there.',
  affiliation: 'Sets affiliation from a country code, by the country lists.',
  tracker: 'Forms tracks from detections (plots).',
  throttle: 'Limits how often each source track is passed on, by time and distance moved.',
  output: 'The source’s system tracks, to correlation. A source has one.',
  nats: 'Publishes what reaches it, as JSON, on its NATS subject, without correlation.',
}

/**
 * The settings of one step, and its samples as they leave it. The input's are the source's
 * transport and codec; the others' are the step's own.
 */
export function NodePanel({
  spec,
  node,
  path,
  problems,
  schema,
  plugins,
  preview,
  previewError,
  probe,
  capturing,
  readOnly,
  onSpec,
  onConfig,
  onCapture,
  onClose,
}: {
  spec: SourceSpec
  node: FlowNode
  /** The steps from the input to this one. */
  path: string[]
  problems: string[]
  schema: SchemaOverview | null
  plugins: PluginInfo[]
  preview: PreviewResult | null
  previewError: string | null
  probe: ProbeResult | null
  capturing: boolean
  readOnly: boolean
  onSpec: (spec: SourceSpec) => void
  onConfig: (config: unknown) => void
  onCapture: () => void
  onClose: () => void
}) {
  const config = (node.config ?? {}) as Obj
  const set = (patch: Obj) => onConfig({ ...config, ...patch })

  let settings: React.ReactNode = null
  switch (node.type) {
    case 'input': {
      const detected = probe && JSON.stringify(probe.codec) !== JSON.stringify(spec.pipeline.codec) ? probe.codec : null
      settings = (
        <div className="stack">
          <div className="form-grid">
            <div className="field">
              <Label size="sm" htmlFor="input-transport">
                Transport
              </Label>
              <Select
                id="input-transport"
                ariaLabel="Transport"
                options={Object.entries(TRANSPORTS).map(([value, t]) => ({ value, label: t.label }))}
                value={spec.transport.type}
                onChange={(v) => v && onSpec(withTransport(spec, v))}
              />
            </div>
          </div>
          <TransportForm
            part="source"
            transport={spec.transport}
            codec={spec.pipeline.codec}
            onTransport={(transport) => onSpec({ ...spec, transport })}
            onCodec={(codec) => onSpec({ ...spec, pipeline: { ...spec.pipeline, codec } })}
          />
          <SenderAuth spec={spec} onChange={onSpec} />
          <div className="form-grid">
            <div className="field">
              <Label size="sm" htmlFor="input-codec">
                Codec
              </Label>
              <Select
                id="input-codec"
                ariaLabel="Codec"
                options={codecChoices(plugins)}
                value={codecChoice(spec.pipeline.codec)}
                onChange={(v) => v && onSpec(withCodec(spec, v, plugins))}
              />
            </div>
          </div>
          {detected && (
            <div className="value-row">
              <Button
                size="sm"
                variant="secondary"
                icon={<TbWand />}
                disabled={readOnly}
                onClick={() => onSpec({ ...spec, pipeline: { ...spec.pipeline, codec: detected } })}
              >
                Use detected settings
              </Button>
              <InfoTip label="Detected settings">
                The captured samples decode with these settings: {JSON.stringify(detected)}. Using them replaces the codec&apos;s settings.
              </InfoTip>
            </div>
          )}
          <TransportForm
            part="decode"
            transport={spec.transport}
            codec={spec.pipeline.codec}
            onTransport={(transport) => onSpec({ ...spec, transport })}
            onCodec={(codec) => onSpec({ ...spec, pipeline: { ...spec.pipeline, codec } })}
          />
        </div>
      )
      break
    }
    case 'reject':
      settings = <RejectForm rules={(node.config as Obj[] | undefined) ?? []} onChange={(rules) => onConfig(rules)} />
      break
    case 'map': {
      const mapping = (config.mapping as Obj | undefined) ?? { rules: [] }
      const suggested = probe?.suggestion.mapping
      const offer = suggested && suggested.rules.length > 0 && JSON.stringify(suggested.rules) !== JSON.stringify(mapping.rules)
      const statics = ((mapping.rules as Obj[] | undefined) ?? []).some((r) => r.kind === 'static')
      settings = (
        <>
          {offer && (
            <div className="value-row">
              <Button size="sm" variant="secondary" icon={<TbWand />} disabled={readOnly} onClick={() => set({ mapping: { ...suggested, reject: undefined } })}>
                Use suggested mapping
              </Button>
              <InfoTip label="Suggested mapping">
                A mapping inferred from the captured samples: {suggested.rules.length} rule(s)
                {probe.suggestion.missing.length ? `; not found in the samples: ${probe.suggestion.missing.join(', ')}` : ''}. Using it replaces
                this step&apos;s rules.
              </InfoTip>
            </div>
          )}
          {schema ? <MapEditor mapping={mapping} schema={schema} onChange={(m) => set({ mapping: m })} /> : <span className="muted">LOADING…</span>}
          {statics && <JoinForm value={(config.static_join as Obj | undefined) ?? {}} onChange={(static_join) => set({ static_join })} />}
        </>
      )
      break
    }
    case 'filter': {
      const input = filterInput(preview?.trace, node.id, path[path.length - 2])
      const fields = sampleFields([...input.observations, ...(preview?.observations ?? [])])
      const key = (o: unknown) => String((o as { source_track_key?: unknown }).source_track_key ?? '')
      const examples = [...new Set(input.dropped.map(key).filter(Boolean))].slice(0, 3)
      settings = <FilterForm value={config} onChange={onConfig} fields={fields} effect={preview ? { dropped: input.dropped.length, examples } : null} />
      break
    }
    case 'registry':
      settings = <RegistryForm value={config} onChange={onConfig} />
      break
    case 'affiliation':
      settings = <AffiliationForm value={config} onChange={onConfig} />
      break
    case 'tracker':
      settings = <TrackerForm value={config} onChange={onConfig} />
      break
    case 'throttle':
      settings = <ThrottleForm value={config} onChange={onConfig} />
      break
    case 'nats':
      settings = (
        <div className="form-grid">
          <div className="field wide">
            <div className="row-label">
              <Label size="sm" htmlFor={`subject-${node.id}`}>
                Subject
              </Label>
              <InfoTip label="Subject">
                The NATS subject to publish on: dot-separated tokens, no wildcards or spaces, outside the system tracks&apos; subjects.
              </InfoTip>
            </div>
            <Input
              id={`subject-${node.id}`}
              className="mono"
              value={String(config.subject ?? '')}
              placeholder="e.g. feeds.ais.raw"
              spellCheck={false}
              onChange={(e) => set({ subject: e.target.value })}
            />
          </div>
        </div>
      )
      break
  }

  const title = nodeTitle(spec, node)
  return (
    <div className="node-panel">
      <div className="node-panel-head">
        <span className="flow-step-kind">{KIND_WORD[node.type]}</span>
        <h3 className="subhead">
          {title}
          <InfoTip label={title}>{STEP_HELP[node.type]}</InfoTip>
        </h3>
        <span className="spacer" />
        <Button size="xs" variant="ghost" icon={<TbX />} aria-label="Close" title="Close" onClick={onClose} />
      </div>
      <div className="node-panel-body">
        {problems.map((m) => (
          <div key={m} className="error-text" role="alert">
            {m}
          </div>
        ))}
        {settings && (
          <fieldset className="step-settings" disabled={readOnly}>
            {settings}
          </fieldset>
        )}
        <div className="value-row">
          <h3 className="subhead">
            Samples
            <InfoTip label="Samples">
              A dry run of the source as edited (not yet saved) over its stored samples, as they leave this step. Nothing is published.
            </InfoTip>
          </h3>
          <span className="spacer" />
          {node.type === 'input' && (
            <>
              <Button size="sm" variant="secondary" icon={<TbDownload />} disabled={capturing || readOnly} onClick={onCapture}>
                {capturing ? 'Capturing…' : 'Capture samples'}
              </Button>
              <InfoTip label="Capture samples">
                Connect with these settings and take up to 20 frames or 15 s of the live feed, whichever comes first. They replace the
                source&apos;s stored samples.
              </InfoTip>
            </>
          )}
        </div>
        {previewError && <div className="error-text">{previewError}</div>}
        {preview && <StageSamples result={preview} stageId={node.type === 'output' ? 'publish' : node.id} path={path} />}
      </div>
    </div>
  )
}
