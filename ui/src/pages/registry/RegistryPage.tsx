import { useCallback, useEffect, useRef, useState } from 'react'
import { TbDownload, TbPlus, TbSearch, TbUpload } from 'react-icons/tb'
import { Badge, Button, CollapsiblePanel, DataTable, Input, useToast, type DataTableColumn } from '@kineticlogic/staresdk'
import { api, type Entity, type SheetImport } from '../../api/client'
import { errorMessage, fmtTime, show } from '../../lib/format'
import { InfoTip } from '../../components/InfoTip'
import { EntityEditor } from './EntityEditor'
import { useCan } from '../../auth/context'
import { FILL_PANEL, usePanelOpen } from '../../lib/panelOpen'

const PAGE = 200

const COLUMNS: DataTableColumn<Entity>[] = [
  { key: 'name', header: 'Name', render: (e) => e.name ?? <span className="muted">{e.id}</span>, sortValue: (e) => e.name ?? e.id },
  {
    key: 'ids',
    header: 'Identifiers',
    mono: true,
    render: (e) => e.identifiers.map((i) => `${i.scheme}:${i.value}`).join('  '),
  },
  {
    key: 'identity',
    header: 'Domain · affiliation',
    width: 180,
    render: (e) => [e.domain, e.affiliation].filter(Boolean).join(' · ') || <span className="muted">—</span>,
    sortValue: (e) => `${e.domain ?? ''}${e.affiliation ?? ''}`,
  },
  {
    key: 'attributes',
    header: 'Attributes',
    render: (e) => e.attributes.map((a) => `${a.key} ${show(a.value)}`).join(' · '),
  },
  {
    key: 'updated',
    header: 'Saved',
    width: 140,
    mono: true,
    render: (e) => (e.updated_at_ms ? fmtTime(e.updated_at_ms) : ''),
    sortValue: (e) => e.updated_at_ms ?? 0,
  },
  {
    key: 'status',
    header: 'Status',
    width: 80,
    render: (e) => <Badge size="sm" color={e.status === 'active' ? 'grey' : 'warning'}>{e.status}</Badge>,
  },
]

const ACTION_COLOR = { create: 'success', update: 'blue', unchanged: 'grey', error: 'danger' } as const

/** The registry: every entity with its identifiers, minimum and attributes; create, edit, and spreadsheet export and import. */
export default function RegistryPage() {
  const { toast } = useToast()
  const canManage = useCan('track_manager')
  const [query, setQuery] = useState('')
  const [rows, setRows] = useState<Entity[] | null>(null)
  const [total, setTotal] = useState(0)
  // The entity open in the editor: an id, 'new', or none.
  const [editing, setEditing] = useState<string | null>(null)
  const [plan, setPlan] = useState<{ file: File; result: SheetImport } | null>(null)
  const [busy, setBusy] = useState(false)
  const picker = useRef<HTMLInputElement>(null)
  const [listOpen, setListOpen] = usePanelOpen('ot.panel.registry')

  const load = useCallback(() => {
    api.registryEntities(query.trim(), PAGE).then(
      (r) => {
        setRows(r.entities)
        setTotal(r.total)
      },
      (e) => toast({ variant: 'error', title: 'Registry', message: errorMessage(e) }),
    )
  }, [query, toast])
  useEffect(() => {
    const t = setTimeout(load, 250)
    return () => clearTimeout(t)
  }, [load])

  const preview = async (file: File) => {
    setBusy(true)
    try {
      setPlan({ file, result: await api.importSheet(file, false) })
    } catch (e) {
      toast({ variant: 'error', title: 'Not a registry sheet', message: errorMessage(e) })
    } finally {
      setBusy(false)
    }
  }
  const apply = async () => {
    if (!plan) return
    setBusy(true)
    try {
      const r = await api.importSheet(plan.file, true)
      if (!r.applied) {
        setPlan({ file: plan.file, result: r })
        toast({ variant: 'error', title: 'Not imported', message: 'Some rows have errors; nothing was written.' })
        return
      }
      toast({ variant: 'success', title: 'Imported', message: `${r.counts.create} created, ${r.counts.update} updated` })
      setPlan(null)
      load()
    } catch (e) {
      toast({ variant: 'error', title: 'Not imported', message: errorMessage(e) })
    } finally {
      setBusy(false)
    }
  }

  const planRows = plan?.result.rows.filter((r) => r.action !== 'unchanged') ?? []
  return (
    <div className="stack fill-page">
      <CollapsiblePanel
        title="Registry"
        badge={rows ? (query.trim() ? `${rows.length.toLocaleString()} of ${total.toLocaleString()}` : total.toLocaleString()) : undefined}
        open={listOpen}
        onOpenChange={setListOpen}
        style={listOpen ? FILL_PANEL : undefined}
        titleActions={
          <div className="title-tools">
            <div className="search">
              <TbSearch aria-hidden />
              <Input
                aria-label="Search the registry"
                placeholder="Name or identifier"
                style={{ paddingLeft: 26 }}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                autoComplete="off"
                spellCheck={false}
              />
            </div>
            <InfoTip label="Registry">
              One row per real-world entity; click a row to edit it. Search matches part of a name or identifier value, or an exact entity id.
              Status: active entities are what tracks resolve to; retired ones are kept on record but no track resolves to them.
            </InfoTip>
          </div>
        }
        actions={
          <>
            <Button size="sm" icon={<TbPlus />} disabled={!canManage} onClick={() => setEditing('new')}>
              New entity
            </Button>
            <Button size="sm" variant="ghost" icon={<TbDownload />} onClick={() => window.open(api.registryExportUrl('xlsx'), '_self')}>
              Export XLSX
            </Button>
            <Button size="sm" variant="ghost" icon={<TbDownload />} onClick={() => window.open(api.registryExportUrl('csv'), '_self')}>
              Export CSV
            </Button>
            <Button size="sm" variant="ghost" icon={<TbUpload />} disabled={!canManage || busy} onClick={() => picker.current?.click()}>
              Import sheet
            </Button>
            <InfoTip label="Registry sheets">
              One row per entity: <span className="mono">entity_id</span>, <span className="mono">name</span>, <span className="mono">status</span>, the
              minimum (<span className="mono">class_name</span>, <span className="mono">domain</span>, <span className="mono">affiliation</span>,{' '}
              <span className="mono">track_type</span>, <span className="mono">cot_type</span>, <span className="mono">sidc</span>),{' '}
              <span className="mono">id:&lt;scheme&gt;</span> (several separated by ;) and <span className="mono">attr:&lt;key&gt;:&lt;type&gt;</span>.
              Export a sheet to start from. Blank cells leave values as they are; an identifier is never taken from another entity.
            </InfoTip>
            <input
              ref={picker}
              type="file"
              accept=".xlsx,.csv"
              hidden
              onChange={(e) => {
                const f = e.target.files?.[0]
                e.target.value = ''
                if (f) preview(f)
              }}
            />
          </>
        }
      >
        <div className="panel-body fill">
          <DataTable
            aria-label="Registry entities"
            columns={COLUMNS}
            rows={rows ?? []}
            rowKey={(e) => e.id}
            onRowClick={(e) => setEditing(e.id)}
            empty={rows ? 'No entity matches.' : 'Loading…'}
            maxHeight="none"
          />
          {rows && total > rows.length && <span className="muted">Showing the first {rows.length.toLocaleString()}; search to narrow.</span>}
        </div>
      </CollapsiblePanel>

      {plan && (
        <CollapsiblePanel
          title={`Import ${plan.file.name}`}
          badge={`${plan.result.counts.create} new · ${plan.result.counts.update} updated · ${plan.result.counts.unchanged} unchanged · ${plan.result.counts.error} errors`}
          persistKey="ot.panel.registryimport"
          actions={
            <>
              <Button size="sm" variant="ghost" onClick={() => setPlan(null)}>
                Cancel
              </Button>
              <Button size="sm" disabled={!canManage || busy || plan.result.counts.error > 0 || planRows.length === 0} onClick={apply}>
                Apply
              </Button>
            </>
          }
        >
          <div className="panel-body">
            {plan.result.counts.error > 0 && <div className="error-text">Fix the rows with errors and import again: nothing is written while any row has one.</div>}
            <DataTable
              aria-label="Import plan"
              columns={[
                { key: 'row', header: 'Row', width: 56, align: 'right', render: (r) => r.row },
                { key: 'action', header: 'Change', width: 96, render: (r) => <Badge size="sm" color={ACTION_COLOR[r.action]}>{r.action}</Badge> },
                { key: 'entity', header: 'Entity', render: (r) => r.name ?? r.entity_id },
                {
                  key: 'what',
                  header: 'What',
                  render: (r) =>
                    r.errors?.length ? (
                      <span className="error-text">{r.errors.join('; ')}</span>
                    ) : (
                      [
                        r.identifiers_added?.length ? `+ ${r.identifiers_added.join(', ')}` : '',
                        r.fields?.length ? r.fields.join(', ') : '',
                      ]
                        .filter(Boolean)
                        .join(' · ')
                    ),
                },
              ]}
              rows={planRows}
              rowKey={(r) => String(r.row)}
              empty="Nothing to change."
              maxHeight={360}
            />
          </div>
        </CollapsiblePanel>
      )}

      <EntityEditor
        entityId={editing === 'new' ? null : editing}
        open={editing !== null}
        onClose={() => setEditing(null)}
        onSaved={(id) => {
          if (editing === 'new' && id) setEditing(id)
          load()
        }}
      />
    </div>
  )
}
