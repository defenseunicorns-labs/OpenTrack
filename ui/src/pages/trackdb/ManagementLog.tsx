import { useCallback, useEffect, useState } from 'react'
import { TbArrowBackUp, TbRefresh } from 'react-icons/tb'
import { Badge, Button, CollapsiblePanel, DataTable, type DataTableColumn } from '@kineticlogic/staresdk'
import { api, UNDOABLE_OPS, type DecisionRow } from '../../api/client'
import { useCan } from '../../auth/context'
import { InfoTip } from '../../components/InfoTip'
import { fmtTime } from '../../lib/format'
import { evidenceTracks, opLabel, undoable, useUndo } from './undo'

const OPS = [...UNDOABLE_OPS, 'undo', 'delete_history_point']
const REFRESH_MS = 15_000

/** Recent track management decisions, newest first, with Undo. */
export function ManagementLog({ rev, onChanged }: { rev: number; onChanged: () => void }) {
  const canManage = useCan('track_manager')
  const undo = useUndo()
  const [rows, setRows] = useState<DecisionRow[] | null>(null)
  const [busy, setBusy] = useState<number | null>(null)
  const [site, setSite] = useState<string | null>(null)
  useEffect(() => {
    api.status().then((s) => setSite(s.site), () => {})
  }, [])
  const load = useCallback(() => {
    api.decisions(OPS, 200).then(setRows, () => setRows((r) => r ?? []))
  }, [])
  useEffect(() => {
    load()
    const t = setInterval(load, REFRESH_MS)
    return () => clearInterval(t)
  }, [load, rev])

  const run = async (d: DecisionRow) => {
    setBusy(d.id)
    try {
      if (await undo(d)) onChanged()
    } finally {
      setBusy(null)
      load()
    }
  }

  const columns: DataTableColumn<DecisionRow>[] = [
    { key: 'id', header: '#', width: 60, mono: true, render: (d) => d.id, sortValue: (d) => d.id },
    { key: 'at', header: 'Time', width: 170, mono: true, render: (d) => fmtTime(d.at_ms), sortValue: (d) => d.at_ms },
    {
      key: 'actor',
      header: 'By',
      width: 190,
      render: (d) => {
        // Made on another node (its global id is <site>:<seq>).
        const from = typeof d.evidence?.sync === 'string' ? d.evidence.sync.split(':')[0] : null
        return (
          <span className="num-row" title={d.actor}>
            <span>{d.actor}</span>
            {from && site && from !== site && (
              <Badge color="grey" size="sm">
                {from}
              </Badge>
            )}
          </span>
        )
      },
      sortValue: (d) => d.actor,
    },
    {
      key: 'op',
      header: 'Decision',
      width: 150,
      sortValue: (d) => d.op,
      render: (d) => (
        <Badge size="sm" color={d.op === 'undo' ? 'warning' : d.op.includes('delete') || d.op === 'dissolve_group' ? 'danger' : 'blue'}>
          {d.op === 'undo' && typeof d.evidence?.op === 'string' ? `Undo ${opLabel(d.evidence.op).toLowerCase()}` : opLabel(d.op)}
        </Badge>
      ),
    },
    {
      key: 'tracks',
      header: 'Tracks',
      width: 220,
      render: (d) => {
        const ts = evidenceTracks(d.evidence)
        return ts.length ? (
          <span className="mono" title={ts.join(', ')}>
            {ts.join(', ')}
          </span>
        ) : (
          <span className="muted">—</span>
        )
      },
    },
    { key: 'reason', header: 'Reason', render: (d) => <span title={d.reason ?? ''}>{d.reason ?? '—'}</span> },
    {
      key: 'status',
      header: 'Status',
      width: 120,
      render: (d) =>
        d.undone_by != null ? (
          <Badge size="sm" color="grey">
            undone by #{d.undone_by}
          </Badge>
        ) : d.undoes != null ? (
          <span className="muted">undoes #{d.undoes}</span>
        ) : null,
    },
    {
      key: 'act',
      header: '',
      width: 80,
      align: 'right',
      render: (d) =>
        undoable(d) ? (
          <Button size="sm" variant="ghost" icon={<TbArrowBackUp />} disabled={!canManage || busy !== null} onClick={() => run(d)} aria-label={`Undo decision ${d.id}`}>
            Undo
          </Button>
        ) : null,
    },
  ]

  return (
    <CollapsiblePanel
      title="Track management log"
      persistKey="ot.panel.trackmanagementlog"
      titleActions={
        <InfoTip label="Track management log">
          Pairings, merges, splits, do-not-pair rules, deletes and group changes, newest first. Undo reverses one as a new decision. It is refused when someone has changed
          those tracks since: undo that decision first. The engine&apos;s own decisions are not undone here.
          <br />
          <br />
          By: who decided; a node badge means it was decided on that other OpenTrack node and shared here. Decision: red for deletes and
          dissolved groups, amber for undos. Status: undone by #N names the decision that reversed this one; undoes #N, the one this
          reversed.
        </InfoTip>
      }
      actions={<Button size="xs" variant="ghost" icon={<TbRefresh />} aria-label="Refresh the log" title="Refresh" onClick={load} />}
    >
      <div className="panel-body">
        <DataTable
          aria-label="Track management decisions"
          columns={columns}
          rows={rows ?? []}
          rowKey={(d) => String(d.id)}
          maxHeight={360}
          defaultSort={{ key: 'id', direction: 'desc' }}
          empty={rows === null ? 'LOADING…' : 'No track management decisions yet.'}
        />
      </div>
    </CollapsiblePanel>
  )
}
