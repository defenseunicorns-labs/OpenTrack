import { useState } from 'react'
import { TbEye, TbEyeOff, TbFocus2, TbTrash } from 'react-icons/tb'
import { Button, DataTable, Input, Modal, useToast, type DataTableColumn } from '@kineticlogic/staresdk'
import { api, type HistoryPoint } from '../../api/client'
import { useCan } from '../../auth/context'
import { InfoTip } from '../../components/InfoTip'
import { errorMessage, fmtNum, fmtTime } from '../../lib/format'
import { INPUT } from '../../lib/valueSpec'

/** Ask for an optional reason, then delete one history point. */
function DeletePoint({ uid, point, onClose, onDone }: { uid: string; point: HistoryPoint; onClose: () => void; onDone: () => void }) {
  const { toast } = useToast()
  const [reason, setReason] = useState('')
  const [busy, setBusy] = useState(false)
  const run = async (e: React.FormEvent) => {
    e.preventDefault()
    setBusy(true)
    try {
      const r = await api.deleteHistoryPoint(uid, point.t, reason.trim() || undefined)
      toast({
        variant: 'success',
        title: 'Point deleted',
        message: r.stepped_back ? 'It was the latest: the track stepped back to the point before and was republished.' : `Decision #${r.decision}.`,
      })
      onDone()
    } catch (err) {
      toast({ variant: 'error', title: 'Not deleted', message: errorMessage(err) })
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal title="Delete history point" onClose={onClose} width={480} resizable={false}>
      <form className="panel-body stack" onSubmit={run}>
        <div className="mono">
          {fmtTime(point.t)} · {point.lat.toFixed(5)}, {point.lon.toFixed(5)}
        </div>
        <div className="settings-row">
          <div className="row-label">
            <span className="field-caps" style={{ marginBottom: 0 }}>
              Reason
            </span>
            <InfoTip label="Reason">Optional. Kept in the decision log.</InfoTip>
          </div>
          <div className="settings-row-control">
            <Input style={{ ...INPUT, width: 260 }} aria-label="Reason" autoFocus placeholder="e.g. multipath jump" value={reason} onChange={(e) => setReason(e.target.value)} />
          </div>
        </div>
        <div className="num-row" style={{ justifyContent: 'flex-end' }}>
          <Button size="sm" variant="ghost" type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" variant="danger" type="submit" icon={<TbTrash />} disabled={busy}>
            Delete point
          </Button>
        </div>
      </form>
    </Modal>
  )
}

/**
 * A track's position history, newest first, with Delete point for a track manager. Above it, Show on
 * map (the history as a line on the track map; off until asked) and Zoom to track.
 */
export function TrackHistory({
  uid,
  points,
  onChanged,
  onMap = false,
  onToggleMap,
  onZoom,
}: {
  uid: string
  points: HistoryPoint[] | null
  onChanged: () => void
  /** The history is drawn on the map. */
  onMap?: boolean
  onToggleMap?: (on: boolean) => void
  onZoom?: () => void
}) {
  const canManage = useCan('track_manager')
  const [deleting, setDeleting] = useState<HistoryPoint | null>(null)
  const rows = points ? [...points].reverse() : []
  const columns: DataTableColumn<HistoryPoint>[] = [
    { key: 't', header: 'Time', width: 170, mono: true, render: (p) => fmtTime(p.t) },
    { key: 'lat', header: 'Lat', align: 'right', mono: true, render: (p) => p.lat.toFixed(5) },
    { key: 'lon', header: 'Lon', align: 'right', mono: true, render: (p) => p.lon.toFixed(5) },
    { key: 'course', header: 'Course', align: 'right', mono: true, render: (p) => fmtNum(p.course, 0, '°') },
    { key: 'speed', header: 'Speed', align: 'right', mono: true, render: (p) => fmtNum(p.speed, 1, ' m/s') },
    {
      key: 'act',
      header: '',
      width: 44,
      align: 'right',
      render: (p) => (
        <Button size="xs" variant="ghost" icon={<TbTrash />} title="Delete point" aria-label={`Delete point at ${fmtTime(p.t)}`} disabled={!canManage} onClick={() => setDeleting(p)} />
      ),
    },
  ]
  return (
    <div className="stack">
      {(onToggleMap || onZoom) && (
        <div className="num-row">
          {onToggleMap && (
            <Button
              size="sm"
              variant="secondary"
              active={onMap}
              aria-pressed={onMap}
              icon={onMap ? <TbEye /> : <TbEyeOff />}
              disabled={!points || points.length === 0}
              onClick={() => onToggleMap(!onMap)}
            >
              {onMap ? 'Shown on map' : 'Show on map'}
            </Button>
          )}
          {onZoom && (
            <Button size="sm" variant="secondary" icon={<TbFocus2 />} onClick={onZoom}>
              Zoom to track
            </Button>
          )}
          <InfoTip label="History on the map">
            Show on map draws this history on the track map as a line in the track&apos;s colour, oldest to newest, ending at its current position. It is
            off for each track until turned on. Zoom to track moves the map to the track: to the whole line when it is shown, otherwise to where the track
            is now.
          </InfoTip>
        </div>
      )}
      <h3 className="subhead num-row">
        Position history
        <InfoTip label="Position history">
          Where the track was, as published, kept for the hours set in Settings. Deleting a bad point is a decision; when it is the latest, the track steps
          back to the point before.
        </InfoTip>
        {points && <span className="muted"> · {points.length.toLocaleString()} points</span>}
      </h3>
      <DataTable
        aria-label="Position history"
        columns={columns}
        rows={rows}
        rowKey={(p) => String(p.t)}
        maxHeight={320}
        empty={points === null ? 'LOADING…' : 'No history kept for this track.'}
      />
      {deleting && (
        <DeletePoint
          uid={uid}
          point={deleting}
          onClose={() => setDeleting(null)}
          onDone={() => {
            setDeleting(null)
            onChanged()
          }}
        />
      )}
    </div>
  )
}
