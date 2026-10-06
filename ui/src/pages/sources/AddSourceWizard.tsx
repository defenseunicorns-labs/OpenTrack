import { useMemo, useState } from 'react'
import { TbArrowLeft, TbArrowRight, TbCheck, TbRadar, TbX } from 'react-icons/tb'
import { Badge, Button, CollapsiblePanel, DataTable, Input, Label, Stepper, Toggle, useToast, type DataTableColumn, type StepStatus } from '@kineticlogic/staresdk'
import { CodeEditor } from '@kineticlogic/staresdk/code-editor'
import { api, type FieldStat, type ProbeResult, type SourceSpec } from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { errorMessage } from '../../lib/format'
import { MappingStudio } from './MappingStudio'
import { needsAcceptance } from '../../lib/senderAuth'
import { SenderAuth } from './SenderAuth'
import { TransportForm } from './TransportForm'

const STEPS = [
  { id: 'connect', label: 'Connect' },
  { id: 'probe', label: 'Probe' },
  { id: 'map', label: 'Map' },
  { id: 'review', label: 'Review' },
] as const

const ID_RE = /^[a-z0-9_-]{1,64}$/

const FIELD_COLUMNS: DataTableColumn<FieldStat>[] = [
  { key: 'path', header: 'Field', mono: true, width: '32%', render: (f) => f.path, sortValue: (f) => f.path },
  { key: 'types', header: 'Types', width: '16%', render: (f) => Object.keys(f.types).join(', ') },
  {
    key: 'presence',
    header: 'Present',
    align: 'right',
    width: '11%',
    render: (f) => `${Math.round(f.presence * 100)}%`,
    sortValue: (f) => f.presence,
  },
  {
    key: 'distinct',
    header: 'Distinct',
    align: 'right',
    width: '11%',
    render: (f) => `${f.distinct}${f.distinct_capped ? '+' : ''}`,
    sortValue: (f) => f.distinct,
  },
  {
    key: 'samples',
    header: 'Samples',
    mono: true,
    render: (f) => f.samples.map((s) => (typeof s === 'string' ? s : JSON.stringify(s))).join(' · '),
  },
]

function connectProblems(spec: SourceSpec): string[] {
  const p: string[] = []
  if (!ID_RE.test(spec.id)) p.push('an id of a-z, 0-9, - or _')
  if (!spec.name.trim()) p.push('a name')
  const t = spec.transport
  if ((t.type === 'http_poll' || t.type === 'websocket') && !String(t.url ?? '').trim()) p.push('a URL')
  if (t.type === 'mqtt') {
    if (!/^mqtts?:\/\/./.test(String(t.url ?? '').trim())) p.push('a broker URL (mqtt:// or mqtts://)')
    if (!Array.isArray(t.topics) || t.topics.length === 0) p.push('at least one topic')
    if (t.clean_session === false && !String(t.client_id ?? '').trim()) p.push('a client id for a persistent session')
  }
  if (t.type === 'tcp_client' && (!String(t.host ?? '').trim() || !t.port)) p.push('a host and port')
  if ((t.type === 'tcp_server' || t.type === 'udp' || t.type === 'grpc_server') && !String(t.bind ?? '').trim()) p.push('a bind address')
  const c = spec.pipeline.codec
  if (t.type === 'grpc_client' || t.type === 'grpc_server' || c.type === 'protobuf') {
    if (c.type !== 'protobuf') p.push('the Protobuf codec (gRPC carries protobuf)')
    else {
      if (Object.keys((c.files as Record<string, string> | undefined) ?? {}).length === 0) p.push("the producer's .proto files")
      if (!String(c.message ?? '').trim()) p.push('the message each frame holds')
    }
  }
  if (t.type === 'grpc_client') {
    if (!/^https?:\/\/./.test(String(t.url ?? '').trim())) p.push('a URL (http:// or https://)')
    if (!String(t.method ?? '').trim()) p.push('the method to call')
  }
  if (needsAcceptance(spec)) p.push(t.type === 'udp' ? 'the acceptance of unauthenticated senders (UDP)' : 'sender authentication, or its risk accepted')
  return p
}

/** Onboard a feed: connect, probe, map, review. No code. */
export function AddSourceWizard({ onDone, onCancel }: { onDone: (id: string) => void; onCancel: () => void }) {
  const { toast } = useToast()
  const [step, setStep] = useState(0)
  const [spec, setSpec] = useState<SourceSpec>({
    id: '',
    name: '',
    transport: { type: 'http_poll', url: '', interval_secs: 5 },
    pipeline: { codec: { type: 'json' }, mapping: { schema_version: 1, rules: [] } },
  })
  const [probe, setProbe] = useState<ProbeResult | null>(null)
  const [probing, setProbing] = useState(false)
  const [maxFrames, setMaxFrames] = useState(50)
  const [maxSecs, setMaxSecs] = useState(20)
  const [enable, setEnable] = useState(true)
  const [saving, setSaving] = useState(false)
  const [saveError, setSaveError] = useState<string | null>(null)

  const problems = connectProblems(spec)
  const probed = !!probe && probe.records > 0
  const statuses: StepStatus[] = STEPS.map((_, i) =>
    i === step ? 'current' : i < step ? (i === 1 && !probed ? 'error' : 'complete') : 'upcoming',
  )

  const runProbe = async () => {
    setProbing(true)
    try {
      const r = await api.probe({
        transport: spec.transport,
        codec: spec.pipeline.codec,
        max_frames: maxFrames,
        max_secs: maxSecs,
        save_as: spec.id,
      })
      setProbe(r)
      // Adopt the detected codec settings and the suggested mapping.
      setSpec((s) => ({ ...s, pipeline: { ...s.pipeline, codec: r.codec, mapping: r.suggestion.mapping } }))
      if (r.frames === 0) {
        toast({ variant: 'warning', message: r.link_error ?? r.hint ?? 'No frames arrived within the probe window.' })
      }
    } catch (e) {
      toast({ variant: 'error', title: 'Probe failed', message: errorMessage(e) })
    } finally {
      setProbing(false)
    }
  }

  const save = async () => {
    setSaving(true)
    setSaveError(null)
    try {
      await api.createSource(spec)
      if (enable) await api.setEnabled(spec.id, true)
      toast({ variant: 'success', message: `Source ${spec.id} saved${enable ? ' and enabled' : ''}.` })
      onDone(spec.id)
    } catch (e) {
      setSaveError(errorMessage(e))
    } finally {
      setSaving(false)
    }
  }

  const specText = useMemo(() => JSON.stringify(spec, null, 2), [spec])
  const canNext = step === 0 ? problems.length === 0 : step === 1 ? probed : true

  return (
    <CollapsiblePanel title="Add source">
      <div className="panel-body">
      <div className="toolbar">
        <Stepper
          aria-label="Add source steps"
          steps={STEPS.map((s, i) => ({ ...s, status: statuses[i] }))}
          onStepClick={(id) => setStep(STEPS.findIndex((s) => s.id === id))}
        />
        <span className="spacer" />
        <Button size="xs" variant="ghost" icon={<TbX />} aria-label="Cancel" title="Cancel" onClick={onCancel} />
      </div>

      {step === 0 && (
        <div className="stack" style={{ gap: 10 }}>
          <div className="form-grid">
            <div className="field">
              <div className="row-label">
                <Label htmlFor="src-id" size="sm">
                  Id
                </Label>
                <InfoTip label="Id">
                  The source&apos;s permanent key: up to 64 of a-z, 0-9, - and _. Its tracks, history and samples are filed under it, so it
                  cannot be changed after saving.
                </InfoTip>
              </div>
              <Input
                id="src-id"
                value={spec.id}
                error={spec.id !== '' && !ID_RE.test(spec.id)}
                placeholder="e.g. harbour-radar"
                onChange={(e) => setSpec({ ...spec, id: e.target.value.trim().toLowerCase() })}
                autoComplete="off"
                spellCheck={false}
              />
            </div>
            <div className="field">
              <div className="row-label">
                <Label htmlFor="src-name" size="sm">
                  Name
                </Label>
                <InfoTip label="Name">What operators see for this source in lists and track details. You can rename it at any time.</InfoTip>
              </div>
              <Input id="src-name" value={spec.name} onChange={(e) => setSpec({ ...spec, name: e.target.value })} autoComplete="off" />
            </div>
          </div>
          <TransportForm
            transport={spec.transport}
            codec={spec.pipeline.codec}
            onTransport={(transport) => setSpec({ ...spec, transport })}
            onCodec={(codec) => setSpec({ ...spec, pipeline: { ...spec.pipeline, codec } })}
          />
          <SenderAuth spec={spec} onChange={setSpec} />
          {problems.length > 0 && <span className="muted">Still needed: {problems.join(', ')}.</span>}
        </div>
      )}

      {step === 1 && (
        <div className="stack" style={{ gap: 10 }}>
          <div className="row">
            <div className="field" style={{ width: 110 }}>
              <div className="row-label">
                <Label htmlFor="max-frames" size="sm">
                  Max frames
                </Label>
                <InfoTip label="Max frames">The probe stops after this many frames (messages) arrive, or at Max seconds, whichever comes first. 1 to 2000.</InfoTip>
              </div>
              <Input id="max-frames" type="number" value={maxFrames} onChange={(e) => setMaxFrames(Number(e.target.value) || 1)} />
            </div>
            <div className="field" style={{ width: 110 }}>
              <div className="row-label">
                <Label htmlFor="max-secs" size="sm">
                  Max seconds
                </Label>
                <InfoTip label="Max seconds">The longest the probe listens, in seconds, if Max frames has not arrived first. 1 to 120.</InfoTip>
              </div>
              <Input id="max-secs" type="number" value={maxSecs} onChange={(e) => setMaxSecs(Number(e.target.value) || 1)} />
            </div>
            <Button size="sm" icon={<TbRadar />} onClick={runProbe} disabled={probing} style={{ alignSelf: 'end' }}>
              {probing ? 'Probing…' : probe ? 'Probe again' : 'Run probe'}
            </Button>
            <span className="muted" style={{ alignSelf: 'end' }}>
              Connects with the settings above, captures frames and infers the schema. Captured frames are kept for
              the mapping preview.
            </span>
          </div>
          {probe && (
            <>
              <div className="counts">
                <Badge color={probe.frames > 0 ? 'blue' : 'danger'} size="sm">
                  frames {probe.frames}
                </Badge>
                <Badge color="grey" size="sm">
                  records {probe.records}
                </Badge>
                {probe.decode_errors > 0 && (
                  <Badge color="danger" size="sm">
                    decode errors {probe.decode_errors}
                  </Badge>
                )}
                <Badge color="grey" size="sm">
                  {probe.seconds}s
                </Badge>
                <InfoTip label="Probe results">
                  Frames: messages received. Records: what the codec decoded from them (one frame can hold many; at most 5000 are analysed).
                  Decode errors: frames the codec could not read. Then the time the probe listened. Up to 200 captured frames are kept for the
                  mapping preview.
                </InfoTip>
                {probe.suggested_records_path && (
                  <span className="muted">
                    records found under <span className="mono">{probe.suggested_records_path}</span>
                  </span>
                )}
              </div>
              {probe.link_error && <div className="error-text">{probe.link_error}</div>}
              {probe.hint && <div className="muted">{probe.hint}</div>}
              {probe.last_decode_error && <div className="error-text">{probe.last_decode_error}</div>}
              <h3 className="subhead">
                Inferred fields
                <InfoTip label="Inferred fields">
                  Every leaf field seen in the decoded records; [*] means each element of an array. Present: the share of records with a non-null
                  value. Distinct: different values seen; a + means counting stopped at 1000.
                </InfoTip>
              </h3>
              <DataTable
                aria-label="Inferred fields"
                columns={FIELD_COLUMNS}
                rows={probe.fields}
                rowKey={(f) => f.path}
                maxHeight={300}
                defaultSort={{ key: 'presence', direction: 'desc' }}
                empty="No fields inferred."
              />
              {probe.sample_frames[0] && (
                <>
                  <h3 className="subhead">First frame</h3>
                  {probe.sample_frames[0].meta?.topic !== undefined && (
                    <span className="muted">
                      topic <span className="mono">{String(probe.sample_frames[0].meta.topic)}</span>, available to the
                      mapping as <span className="mono">_frame.topic</span> and{' '}
                      <span className="mono">_frame.topic_levels[n]</span>
                    </span>
                  )}
                  <pre className="frame">{probe.sample_frames[0].text}</pre>
                </>
              )}
            </>
          )}
        </div>
      )}

      {step === 2 && (
        <MappingStudio
          spec={spec}
          onChange={setSpec}
          sampleSourceId={spec.id}
          proposals={probe?.suggestion.proposals}
          missing={probe?.suggestion.missing}
        />
      )}

      {step === 3 && (
        <div className="stack" style={{ gap: 10 }}>
          <CodeEditor aria-label="Source spec" value={specText} readOnly minHeight={160} maxHeight={420} />
          <label className="row" style={{ gap: 8 }}>
            <Toggle size="sm" value={enable} onChange={setEnable} aria-label="Enable after saving" />
            <span>Enable after saving (starts the source and publishes its tracks)</span>
          </label>
          {saveError && <div className="error-text">{saveError}</div>}
        </div>
      )}

      <div className="footer-actions">
        {step > 0 && (
          <Button size="sm" variant="secondary" icon={<TbArrowLeft />} onClick={() => setStep(step - 1)}>
            Back
          </Button>
        )}
        {step < STEPS.length - 1 ? (
          <Button size="sm" icon={<TbArrowRight />} onClick={() => setStep(step + 1)} disabled={!canNext}>
            Next
          </Button>
        ) : (
          <Button size="sm" icon={<TbCheck />} onClick={save} disabled={saving}>
            {saving ? 'Saving…' : 'Save source'}
          </Button>
        )}
      </div>
      </div>
    </CollapsiblePanel>
  )
}
