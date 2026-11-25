import { lazy, Suspense, useCallback, useEffect, useState } from 'react'
import { TbPlus, TbSearch } from 'react-icons/tb'
import { Badge, Button, DataTable, Input, type DataTableColumn } from '@kineticlogic/staresdk'
import { api, type SourceRow } from '../../api/client'
import { ago, errorMessage, fmtCount } from '../../lib/format'
import { sourceState } from '../../lib/sourceState'
import { NewSource } from './NewSource'
import { UnauthenticatedBadge } from './SenderAuth'
import { useCan } from '../../auth/context'
import { InfoTip } from '../../components/InfoTip'
import { FILL_TABLE, useFillHeight } from '../../lib/fillHeight'

// React Flow loads only with the Sources page's topology and the source editor.
const Topology = lazy(() => import('./Topology'))
const SourceEditor = lazy(() => import('./SourceEditor'))

const REFRESH_MS = 5000
const TABLE_STYLE = { ...FILL_TABLE, border: 0, borderRadius: 0 } as const

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
  const [query, setQuery] = useState('')
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

  const term = query.trim().toLowerCase()
  const rows = sources?.filter((source) => !term || [source.name, source.id].some((value) => value.toLowerCase().includes(term))) ?? []

  return (
    <div className="sources-overview fill-page">
      <section aria-labelledby="source-topology-heading">
        <div className="toolbar">
          <h2 className="subhead" id="source-topology-heading">Topology</h2>
          <InfoTip label="Topology">
            Follow each source from left to right to see how OpenTrack handles its messages and where it sends them.
            Moving dashes show links that carried data. Gray dashed links belong to sources that are switched off.
            Rates show messages per minute, averaged over the last 10 minutes. Select a source or one of its steps to open it.
          </InfoTip>
        </div>
        <Suspense fallback={<span className="muted">LOADING…</span>}>
          {sources ? <Topology sources={sources} onSelect={onSelect} /> : !error && <span className="muted">LOADING…</span>}
        </Suspense>
      </section>
      <section className="panel-fill source-list" aria-labelledby="sources-list-heading" ref={listFill}>
        <div className="toolbar">
          <h2 className="subhead" id="sources-list-heading">Sources</h2>
          <div className="search">
            <TbSearch aria-hidden />
            <Input
              aria-label="Search sources"
              placeholder="Name or source ID"
              style={{ paddingLeft: 26 }}
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              autoComplete="off"
              spellCheck={false}
            />
          </div>
          <InfoTip label="Sources columns">
            <div>Select a source to open it. Unauthenticated means the source accepts messages without checking who sent them.</div>
            <div>Emitted counts updates sent since the source last started. Last frame tells you how long ago a message arrived.</div>
            <div>
              A draft is unfinished. A disabled source is switched off. Running means the source is connected or listening.
              Starting and connecting mean it is still getting ready. Failing means it hit connection errors and will retry.
              Not started means a problem with the setup kept it from starting.
            </div>
          </InfoTip>
          {sources && <Badge size="sm" color="grey">{term ? `${rows.length} of ${sources.length}` : sources.length}</Badge>}
          <span className="spacer" />
          <Button size="sm" icon={<TbPlus />} disabled={!canAdmin} onClick={() => setAdding(true)}>
            Add source
          </Button>
        </div>
        {error && <div className="error-text" role="alert">{error}</div>}
        <DataTable
          aria-label="Sources"
          columns={COLUMNS}
          rows={rows}
          rowKey={(s) => s.id}
          onRowClick={(s) => onSelect(s.id)}
          maxHeight={listHeight ?? 'none'}
          style={TABLE_STYLE}
          empty={sources ? term ? 'No sources match.' : 'No sources yet.' : error ? 'Sources unavailable.' : 'LOADING…'}
        />
      </section>
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
