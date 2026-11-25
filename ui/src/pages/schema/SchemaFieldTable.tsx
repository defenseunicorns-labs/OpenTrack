import { useState, type ReactNode } from 'react'
import { TbSearch, TbStarFilled } from 'react-icons/tb'
import { Badge, DataTable, FieldSelect, Input, Modal, type DataTableColumn } from '@kineticlogic/staresdk'
import { InfoTip } from '../../components/InfoTip'
import { BACKGROUND_TABLE, FIELD_TYPES, type FieldInput, type SchemaFieldRow } from './schemaFields'

const NO_FEED = ':no-feed'
const OPENTRACK = ':opentrack'

function sourceIds(row: SchemaFieldRow) {
  return [...new Set(row.inputs.map((input) => input.source))]
}

function filledBy(row: SchemaFieldRow) {
  if (row.field?.builtin) return `OpenTrack ${row.field.builtin}`
  return row.core ? 'OpenTrack' : 'feed mapping or entity link'
}

const FIELD_COLUMNS: DataTableColumn<SchemaFieldRow>[] = [
  { key: 'key', header: 'Field', mono: true, width: '19%', sortValue: (f) => f.key, render: (f) => (
    <span className="schema-field-name">
      {f.core && <span className="schema-field-star" role="img" aria-label="Always published"><TbStarFilled aria-hidden /></span>}
      {f.key}
    </span>
  ) },
  { key: 'type', header: 'Type', width: '9%', render: (f) => f.type ?? '—', sortValue: (f) => f.type ?? '' },
  { key: 'unit', header: 'Unit', width: '6%', render: (f) => f.unit ?? '' },
  { key: 'source', header: 'Filled by', width: '18%', render: filledBy, sortValue: filledBy },
  { key: 'used', header: 'Fed by', width: '16%', render: (f) => sourceIds(f).join(', ') ||
    <span className="muted">{f.core || f.field?.builtin ? '—' : 'no feed yet'}</span> },
  { key: 'desc', header: 'Notes', render: (f) => f.field?.description ?? f.field?.enum_values?.join(' | ') ?? '' },
]

const INPUT_COLUMNS: DataTableColumn<FieldInput>[] = [
  { key: 'source', header: 'Source', render: (i) => i.source },
  { key: 'from', header: 'Input', render: (i) => i.from },
  { key: 'target', header: 'OpenTrack field', mono: true, render: (i) => i.target },
  { key: 'stage', header: 'Stage', render: (i) => i.stage },
  { key: 'version', header: 'Schema version', width: 115, render: (i) => i.version },
]

function FieldDetails({ row, onClose }: { row: SchemaFieldRow; onClose: () => void }) {
  return (
    <Modal title={row.key} width={760} onClose={onClose}>
      <div className="panel-body">
        {row.core && <div><Badge size="sm" color="blue">Always published</Badge></div>}
        <dl className="facts">
          <dt>Output field</dt><dd className="mono">{row.id}</dd>
          {row.type && <><dt>Type</dt><dd>{row.type}</dd></>}
          {row.unit && <><dt>Unit</dt><dd>{row.unit}</dd></>}
          <dt>Filled by</dt><dd>{filledBy(row)}</dd>
          {row.targets.length > 0 && <><dt>OpenTrack fields</dt><dd className="mono">{row.targets.join(', ')}</dd></>}
          {row.field?.description && <><dt>Notes</dt><dd>{row.field.description}</dd></>}
          {row.field?.enum_values?.length ? <><dt>Allowed values</dt><dd>{row.field.enum_values.join(', ')}</dd></> : null}
        </dl>
        <div className="toolbar">
          <h3 className="subhead">Fed by</h3>
          <InfoTip label="Fed by">
            The table shows where configured sources get this value and which schema version each mapping was checked against.
            {row.core || row.field?.builtin
              ? ' OpenTrack uses the available inputs to fill this field.'
              : ' If a source links this field to a registry value, OpenTrack uses that value when the entity matches.'}
          </InfoTip>
        </div>
        <DataTable
          aria-label="Field sources"
          columns={INPUT_COLUMNS}
          rows={row.inputs}
          rowKey={(i) => `${i.source}:${i.id}`}
          empty="No source mappings."
          style={BACKGROUND_TABLE}
        />
      </div>
    </Modal>
  )
}

export function SchemaFieldTable({ rows, loaded, visible, toolbarStart, actions }: {
  rows: SchemaFieldRow[]
  loaded: boolean
  visible: boolean
  toolbarStart?: ReactNode
  actions?: ReactNode
}) {
  const [query, setQuery] = useState('')
  const [type, setType] = useState('all')
  const [source, setSource] = useState('all')
  const [picked, setPicked] = useState<string | null>(null)
  const term = query.trim().toLowerCase()
  const sources = [...new Set(rows.flatMap(sourceIds))].sort()
  const selected = rows.find((row) => row.id === picked)
  const filtered = rows.filter((row) => {
    const ids = sourceIds(row)
    return (type === 'all' || row.type === type) &&
      (source === 'all' ||
        (source === NO_FEED && !ids.length && !row.core && !row.field?.builtin) ||
        (source === OPENTRACK && (row.core || !!row.field?.builtin)) || ids.includes(source)) &&
      (!term || [row.key, row.id, row.type, row.unit, row.field?.description,
        row.field?.builtin, ...ids, ...row.targets].some((item) => item?.toLowerCase().includes(term)))
  })
  return (
    <>
      <div className="toolbar">
        {toolbarStart}
        <div className="search">
          <TbSearch aria-hidden />
          <Input aria-label="Search fields" placeholder="Search fields" style={{ paddingLeft: 26 }} value={query} onChange={(e) => setQuery(e.target.value)} />
        </div>
        <FieldSelect ariaLabel="Filter by type" fields={[{ name: 'all', label: 'All types' }, ...FIELD_TYPES.map((name) => ({ name }))]} value={type} onChange={(value) => setType(value ?? 'all')} />
        <FieldSelect
          ariaLabel="Filter by source"
          fields={[{ name: 'all', label: 'All sources' }, { name: OPENTRACK, label: 'OpenTrack' }, { name: NO_FEED, label: 'No feed yet' }, ...sources.map((name) => ({ name }))]}
          value={source}
          onChange={(value) => setSource(value ?? 'all')}
        />
        {actions && <><span className="spacer" />{actions}</>}
      </div>
      <DataTable
        aria-label="Output schema fields"
        columns={FIELD_COLUMNS}
        rows={filtered}
        rowKey={(row) => row.id}
        selectedKey={picked}
        onRowClick={(row) => setPicked(row.id)}
        empty={!loaded ? 'LOADING…' : rows.length ? 'No matching fields.' : 'No fields.'}
        style={BACKGROUND_TABLE}
      />
      {visible && selected && <FieldDetails row={selected} onClose={() => setPicked(null)} />}
    </>
  )
}
