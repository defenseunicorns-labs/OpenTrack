import { useEffect, useRef, useState } from 'react'
import { TbSend } from 'react-icons/tb'
import { Badge, Button, DataTable, FieldSelect, Modal, type DataTableColumn } from '@kineticlogic/staresdk'
import { CodeEditor } from '@kineticlogic/staresdk/code-editor'
import { api, type ExtensionField, type TrackRow } from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { errorMessage } from '../../lib/format'
import { BACKGROUND_TABLE, type SchemaChange } from './schemaFields'

const CHANGE_COLUMNS: DataTableColumn<SchemaChange>[] = [
  { key: 'kind', header: 'Change', width: 130, render: (r) => (
    <Badge size="sm" color={r.kind === 'Removed' ? 'warning' : r.kind === 'Added' ? 'success' : 'blue'}>{r.kind}</Badge>
  ) },
  { key: 'field', header: 'Field', mono: true, render: (r) => r.kind === 'Possible rename'
    ? `${r.before?.key} → ${r.after?.key}`
    : r.after?.key ?? r.before?.key },
  { key: 'details', header: 'Details', render: (r) => r.details?.join(' · ') ?? '—' },
  { key: 'sources', header: 'Mappings to review', render: (r) => {
    const inputs = [...(r.before?.inputs ?? []), ...(r.after?.inputs ?? [])]
    return [...new Set(inputs.map(({ source, version }) => `${source} (v${version})`))].join(', ') || '—'
  } },
]

export function SchemaPublishReview({ version, changes, publishing, onPublish, onClose, error }: {
  version: number
  changes: SchemaChange[]
  publishing: boolean
  onPublish: () => void
  onClose: () => void
  error: string | null
}) {
  return (
    <Modal
      title="Publish schema"
      width={920}
      onClose={onClose}
      titleActions={<Button size="sm" icon={<TbSend />} onClick={onPublish} disabled={publishing}>Publish</Button>}
    >
      <div className="panel-body">
        <div className="toolbar">
          <strong>Draft version {version}</strong>
          <InfoTip label="Publish schema">
            Publishing makes this the live version and locks its fields. OpenTrack sends a fresh update for tracks whose output changes.
            The table compares this draft with the live version. A possible rename has the same field settings under a new name.
            Mappings listed here supply the affected fields.
          </InfoTip>
        </div>
        {error && <div className="error-text" role="alert">{error}</div>}
        <DataTable aria-label="Schema changes" columns={CHANGE_COLUMNS} rows={changes} rowKey={(r) => r.after?.id ?? r.before!.id} empty="No field changes." style={BACKGROUND_TABLE} />
      </div>
    </Modal>
  )
}

export function SchemaOutputPreview({ fields, onClose }: { fields: ExtensionField[]; onClose: () => void }) {
  const [tracks, setTracks] = useState<TrackRow[] | null>(null)
  const [track, setTrack] = useState<string | null>(null)
  const [output, setOutput] = useState<Awaited<ReturnType<typeof api.previewSchema>> | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)
  const requestId = useRef(0)

  useEffect(() => {
    let active = true
    api.tracks(200).then(({ tracks }) => {
      if (!active) return
      setTracks(tracks)
      setTrack(tracks.find((row) => row.published)?.uid ?? tracks[0]?.uid ?? null)
    }, (e) => { if (active) setError(errorMessage(e)) })
    return () => { active = false }
  }, [])

  const preview = async () => {
    if (!track) return
    const request = ++requestId.current
    setLoading(true)
    setOutput(null)
    setError(null)
    try {
      const result = await api.previewSchema(track, fields)
      if (request === requestId.current) setOutput(result)
    } catch (e) {
      if (request === requestId.current) setError(errorMessage(e))
    } finally {
      if (request === requestId.current) setLoading(false)
    }
  }

  return (
    <Modal title="Preview output" onClose={onClose} width={960}>
      <div className="panel-body">
        <div className="toolbar">
          <FieldSelect
            ariaLabel="Sample track"
            fields={(tracks ?? []).map(({ uid, track_id, gold_name }) => ({ name: uid, label: `${track_id} · ${gold_name}` }))}
            value={track}
            onChange={(uid) => { requestId.current++; setTrack(uid); setOutput(null); setError(null); setLoading(false) }}
            style={{ maxWidth: '100%' }}
          />
          <Button size="sm" onClick={() => void preview()} disabled={!track || loading}>Preview</Button>
          <InfoTip label="Preview output">
            Both messages use the same live track. Published uses the live schema and Draft uses the changes you are editing.
            A field only appears when the track has a value for it. Previewing does not save the draft or send a track update.
          </InfoTip>
        </div>
        {error && <div className="error-text" role="alert">{error}</div>}
        {((!tracks && !error) || loading) && <span className="muted" role="status">LOADING…</span>}
        {tracks?.length === 0 && <span className="muted">No live tracks.</span>}
        {output && (
          <div className="schema-preview">
            {(['published', 'draft'] as const).map((side) => {
              const label = side === 'published' ? 'Published' : 'Draft'
              return (
                <section key={side} className="stack">
                  <h3 className="subhead">{label}</h3>
                  <CodeEditor aria-label={`${label} output`} value={JSON.stringify(output[side], null, 2)} readOnly minHeight={260} maxHeight={460} />
                </section>
              )
            })}
          </div>
        )}
      </div>
    </Modal>
  )
}
