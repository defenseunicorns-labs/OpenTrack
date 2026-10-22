import { lazy, Suspense, useCallback, useEffect, useState } from 'react'
import { TbPlus } from 'react-icons/tb'
import { Badge, Button, CollapsiblePanel, DataTable, type DataTableColumn } from '@kineticlogic/staresdk'
import { api, type SourceRow } from '../../api/client'
import { ago, errorMessage, fmtCount } from '../../lib/format'
import { SOURCE_STATES_INFO, sourceState } from '../../lib/sourceState'
import { NewSource } from './NewSource'
import { UnauthenticatedBadge } from './SenderAuth'
import { useCan } from '../../auth/context'
import { InfoTip } from '../../components/InfoTip'
import { FILL_PANEL, usePanelOpen } from '../../lib/panelOpen'
import { FILL_TABLE, useFillHeight } from '../../lib/fillHeight'

// React Flow loads only with the Sources page's topology and the source editor.
const Topology = lazy(() => import('./Topology'))
const SourceEditor = lazy(() => import('./SourceEditor'))

const REFRESH_MS = 5000

const COLUMNS: DataTableColumn<SourceRow>[] = [
  {
    key: 'name',
    header: 'Source',
    render: (s) => (
      <span>
        {s.name} <span className="muted mono">{s.id}</span> <UnauthenticatedBadge spec={s.spec} />
      </span>
    ),
    sortValue: (s) => s.name,
  },
  { key: 'transport', header: 'Transport', width: 110, render: (s) => s.transport, sortValue: (s) => s.transport },
  {
    key: 'state',
    header: 'State',
    width: 100,
    render: (s) => {
      const st = sourceState(s)
      return (
        <Badge color={st.color} size="sm" uppercase>
          {st.label}
        </Badge>
      )
    },
    sortValue: (s) => sourceState(s).label,
  },
  {
    key: 'emitted',
    header: 'Emitted',
    width: 100,
    align: 'right',
    render: (s) => fmtCount(s.status?.totals_since_start.emitted),
    sortValue: (s) => s.status?.totals_since_start.emitted ?? null,
  },
  {
    key: 'last',
    header: 'Last frame',
    width: 100,
    align: 'right',
    render: (s) => ago(s.status?.link.last_frame_at),
  },
]

export default function SourcesPage({ selected, onSelect }: { selected: string; onSelect: (id: string) => void }) {
  const canAdmin = useCan('admin')
  const [sources, setSources] = useState<SourceRow[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const [listOpen, setListOpen] = usePanelOpen('ot.panel.sources')
  const [listFill, listHeight] = useFillHeight()

  const load = useCallback(() => {
    api.sources().then(
      (s) => {
        setSources(s)
        setError(null)
      },
      (e) => setError(errorMessage(e)),
    )
  }, [])

  // The list refreshes while it is shown; the editor keeps its own source fresh.
  useEffect(() => {
    if (selected) return
    load()
    const t = setInterval(load, REFRESH_MS)
    return () => clearInterval(t)
  }, [load, selected])

  if (selected) {
    return (
      <Suspense fallback={<span className="muted">LOADING…</span>}>
        <SourceEditor
          key={selected}
          id={selected}
          onClose={() => onSelect('')}
          onDeleted={() => onSelect('')}
        />
      </Suspense>
    )
  }

  return (
    <div className="panels fill-page">
      <CollapsiblePanel
        title="Topology"
        persistKey="ot.panel.topology"
        titleActions={
          <InfoTip label="Topology">
            Each source&apos;s pipeline, left to right: decode, map, the stages it has, into correlation, the track writer and NATS
            (plus any raw feed). Rates are per minute, averaged over the last 10 minutes; moving dashes mark links that carried data,
            grey dashed links belong to disabled sources. Click a source to open it.
          </InfoTip>
        }
      >
        <div className="panel-body">
          <Suspense fallback={<span className="muted">LOADING…</span>}>
            {sources ? <Topology sources={sources} onSelect={onSelect} /> : <span className="muted">LOADING…</span>}
          </Suspense>
        </div>
      </CollapsiblePanel>
      <CollapsiblePanel
        title="Sources"
        badge={sources ? String(sources.length) : undefined}
        open={listOpen}
        onOpenChange={setListOpen}
        style={listOpen ? FILL_PANEL : undefined}
        titleActions={
          <InfoTip label="Sources columns">
            Source: an UNAUTHENTICATED badge marks a listener that runs without authenticating its senders, with that risk accepted on the
            source. State: {SOURCE_STATES_INFO} Emitted: track updates the source has sent on since its worker last started (a restart or a saved
            change resets it). Last frame: how long ago a message last arrived.
          </InfoTip>
        }
      >
        <div className="panel-body fill" ref={listFill}>
          <div className="toolbar">
            <span className="muted">Feeds OpenTrack ingests. Select one to open it.</span>
            <span className="spacer" />
            <Button size="sm" icon={<TbPlus />} disabled={!canAdmin} onClick={() => setAdding(true)}>
              Add source
            </Button>
          </div>
          {error && <div className="error-text">{error}</div>}
          <DataTable
            aria-label="Sources"
            columns={COLUMNS}
            rows={sources ?? []}
            rowKey={(s) => s.id}
            onRowClick={(s) => onSelect(s.id)}
            maxHeight={listHeight ?? 'none'}
            style={FILL_TABLE}
            empty={sources ? 'No sources yet. Add one to start onboarding a feed.' : 'LOADING…'}
          />
        </div>
      </CollapsiblePanel>
      {adding && (
        <NewSource
          onClose={() => setAdding(false)}
          onCreated={(id) => {
            setAdding(false)
            onSelect(id)
          }}
        />
      )}
    </div>
  )
}
