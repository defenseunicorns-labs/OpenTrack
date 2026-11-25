import { useRef, useState } from 'react'
import { TbAdjustments, TbPlus, TbSettings, TbTrash } from 'react-icons/tb'
import { Button, DataTable, Disclosure, FieldSelect, Input, Label, type DataTableColumn } from '@kineticlogic/staresdk'
import type { SchemaOverview } from '../../../api/client'
import { describeValue, type ValueSpec } from '../../../lib/pipeline'
import type { SampleField } from '../../../lib/conditions'
import { INPUT, toForm } from '../../../lib/valueSpec'
import { InfoTip } from '../../../components/InfoTip'
import { JsonField } from './JsonField'
import { ValueEditor } from './ValueEditor'
import { ConditionBuilder } from './ConditionBuilder'

type Obj = Record<string, unknown>
const OTHER_EXT = 'other ext field…'
const OTHER_ENTITY = 'entity field…'
const KINDS = ['observation', 'static', 'track']

interface Row {
  id: string
  kind: 'key' | 'identifier' | 'field'
  /** Field target, or the identifier's scheme. */
  target: string
  spec: ValueSpec | undefined
  index: number
}

/** The feed field a spec reads when it is a plain path (editable in the table), else null. */
function plainPath(spec: ValueSpec | undefined): string | null {
  if (spec === undefined || typeof spec === 'string') return spec ?? ''
  return toForm(spec).mode === 'path' ? String(spec.path ?? '') : null
}

const withPath = (spec: ValueSpec | undefined, path: string): ValueSpec =>
  spec === undefined || typeof spec === 'string' ? path : { ...spec, path }

/** Whether a spec does more than read a field (transforms, a table, a default...). */
const hasExtras = (spec: ValueSpec | undefined) =>
  spec !== undefined && typeof spec !== 'string' && Object.keys(spec).some((k) => k !== 'path')

/** Pick a mapping target: an OpenTrack field, an `ext.` field of the mapping's schema version, or an
 *  entity field (`entity.<key>`, which updates the track's entity at a corroborated match). */
function TargetPicker({ value, onChange, options }: { value: string; onChange: (t: string) => void; options: string[] }) {
  const typed = (v: string) => (v.startsWith('entity.') ? OTHER_ENTITY : v !== '' && !options.includes(v) ? OTHER_EXT : null)
  const [other, setOther] = useState<string | null>(typed(value))
  const prefix = other === OTHER_ENTITY ? 'entity.' : 'ext.'
  return (
    <div className="value-row" style={{ flexWrap: 'nowrap' }}>
      <FieldSelect
        ariaLabel={`Destination of ${value || 'new field'}`}
        allowNone
        fields={[...options, OTHER_EXT, OTHER_ENTITY].map((name) => ({ name }))}
        value={other ?? (value || null)}
        onChange={(v) => {
          const next = v === OTHER_EXT || v === OTHER_ENTITY ? v : null
          setOther(next)
          const p = v === OTHER_ENTITY ? 'entity.' : 'ext.'
          onChange(next ? (value.startsWith(p) ? value : p) : (v ?? ''))
        }}
        style={{ width: other ? 150 : '100%' }}
      />
      {other && (
        <Input
          style={INPUT}
          aria-label={other === OTHER_ENTITY ? 'entity field name' : 'ext field name'}
          value={value}
          placeholder={`${prefix}key`}
          onChange={(e) => onChange(e.target.value)}
          spellCheck={false}
        />
      )}
    </div>
  )
}

/**
 * The Map stage: a rule's values as a Source → Destination table, edited in place. Click a row
 * for its transforms, default, lookup table or a richer expression below the table.
 */
export function MapEditor({ mapping, onChange, schema, sampleFields }: {
  mapping: Obj
  onChange: (m: Obj) => void
  schema: SchemaOverview
  sampleFields: SampleField[]
}) {
  const rules = (mapping.rules as Obj[]) ?? []
  const [active, setActive] = useState(0)
  const [selected, setSelected] = useState<string | null>(null)
  const [settings, setSettings] = useState(false)
  const [newField, setNewField] = useState<ValueSpec | undefined>(undefined)
  const ruleSettings = useRef<HTMLFieldSetElement>(null)
  // Bumped when rows or rules are removed, so editors reload their values.
  const [epoch, setEpoch] = useState(0)
  // Bumped when the table edits a source, so the field settings show the new value.
  const [inlineRev, setInlineRev] = useState(0)
  const i = Math.min(active, Math.max(rules.length - 1, 0))
  const rule = rules[i]

  const version = Number(mapping.schema_version ?? 1)
  const extFields = (schema.versions.find((v) => v.version === version)?.fields ?? []).filter((f) => !f.builtin).map((f) => `ext.${f.key}`)
  const targets = [...schema.core, ...extFields]

  const setRule = (patch: Obj) => onChange({ ...mapping, rules: rules.map((r, j) => (j === i ? { ...r, ...patch } : r)) })
  const replaceRule = (next: Obj) => onChange({ ...mapping, rules: rules.map((r, j) => (j === i ? next : r)) })
  const drop = (o: Obj, k: string) => Object.fromEntries(Object.entries(o).filter(([x]) => x !== k))
  const fields = Object.entries((rule?.fields as Obj) ?? {})
  const identifiers = (rule?.identifiers as Obj[]) ?? []
  const coordinate = (aliases: string[]) => sampleFields.find((f) => aliases.includes(f.path.toLowerCase()))?.path ?? ''
  const positionFields = (existing: Obj) => existing['geometry.polygon'] !== undefined ? existing : {
    'position.latitude': coordinate(['lat', 'latitude', 'position.lat', 'position.latitude']),
    'position.longitude': coordinate(['lon', 'lng', 'longitude', 'position.lon', 'position.lng', 'position.longitude']),
    ...existing,
  }

  const rows: Row[] = rule
    ? [
        { id: 'key', kind: 'key', target: 'track key', spec: rule.key as ValueSpec, index: 0 },
        ...identifiers.map((x, j): Row => ({ id: `id:${j}`, kind: 'identifier', target: String(x.scheme ?? ''), spec: x.value as ValueSpec, index: j })),
        ...fields.map(([t, v], j): Row => ({ id: `f:${t}`, kind: 'field', target: t, spec: v as ValueSpec, index: j })),
        ...(newField === undefined ? [] : [{ id: 'new-field', kind: 'field' as const, target: '', spec: newField, index: fields.length }]),
      ]
    : []

  /** Replace a row's value spec. */
  const setSpec = (row: Row, spec: ValueSpec) => {
    if (row.id === 'new-field') {
      setNewField(spec)
      return
    }
    if (row.kind === 'key') setRule({ key: spec })
    else if (row.kind === 'identifier') setRule({ identifiers: identifiers.map((x, k) => (k === row.index ? { ...x, value: spec } : x)) })
    else setRule({ fields: Object.fromEntries(fields.map(([k, v]) => [k, k === row.target ? spec : v])) })
  }
  const setTarget = (row: Row, t: string) => {
    if (row.kind === 'identifier') {
      setRule({ identifiers: identifiers.map((x, k) => (k === row.index ? { ...x, scheme: t } : x)) })
      return
    }
    if (t !== row.target && fields.some(([k]) => k === t)) return
    if (row.id === 'new-field') {
      if (!t || t === 'ext.' || t === 'entity.') return
      setRule({ fields: { ...Object.fromEntries(fields), [t]: row.spec ?? '' } })
      setNewField(undefined)
      setSelected(`f:${t}`)
      return
    }
    setRule({ fields: Object.fromEntries(fields.map(([k, v]) => (k === row.target ? [t, v] : [k, v]))) })
    if (selected === row.id) setSelected(`f:${t}`)
  }
  const remove = (row: Row) => {
    if (row.id === 'new-field') setNewField(undefined)
    else if (row.kind === 'identifier') setRule({ identifiers: identifiers.filter((_, k) => k !== row.index) })
    else setRule({ fields: Object.fromEntries(fields.filter(([k]) => k !== row.target)) })
    setSelected(null)
    setEpoch(epoch + 1)
  }

  const columns: DataTableColumn<Row>[] = [
    {
      key: 'source',
      header: 'Source',
      render: (r) => {
        const path = plainPath(r.spec)
        return path === null ? (
          <span className="mono muted" title={describeValue(r.spec)}>
            {describeValue(r.spec)}
          </span>
        ) : (
          <Input style={{ ...INPUT, width: '100%' }} aria-label={`Source of ${r.target || 'new field'}`} value={path} placeholder="feed field" onChange={(e) => {
              setSpec(r, withPath(r.spec, e.target.value))
              setInlineRev((n) => n + 1)
            }}
            spellCheck={false}
          />
        )
      },
    },
    {
      key: 'destination',
      header: 'Destination',
      render: (r) =>
        r.kind === 'key' ? (
          <span className="mono">track key</span>
        ) : r.kind === 'identifier' ? (
          <div className="value-row" style={{ flexWrap: 'nowrap' }}>
            <span className="mono muted">identifier</span>
            <Input style={INPUT} aria-label={`Identifier ${r.index + 1} scheme`} value={r.target} placeholder="scheme" onChange={(e) => setTarget(r, e.target.value)} spellCheck={false} />
          </div>
        ) : (
          <TargetPicker key={`${r.id}:${epoch}`} value={r.target} options={targets} onChange={(t) => setTarget(r, t)} />
        ),
    },
    {
      key: 'actions',
      header: '',
      width: 64,
      align: 'right',
      render: (r) => (
        <span className="value-row" style={{ flexWrap: 'nowrap', justifyContent: 'flex-end' }}>
          {hasExtras(r.spec) && <TbAdjustments aria-label="Has more settings" className="muted" />}
          {r.kind !== 'key' && (
            <Button
              size="xs"
              variant="ghost"
              icon={<TbTrash />}
              aria-label={`Remove ${r.target || 'row'}`}
              title="Remove"
              onClick={(e) => {
                e.stopPropagation()
                remove(r)
              }}
            />
          )}
        </span>
      ),
    },
  ]

  const row = rows.find((r) => r.id === selected) ?? null
  return (
    <div className="stack">
      <div className="value-row">
        <FieldSelect
          ariaLabel="Mapping rule"
          fields={rules.map((r, j) => ({ name: `${j + 1}. ${String(r.name || 'rule')}` }))}
          value={rule ? `${i + 1}. ${String(rule.name || 'rule')}` : null}
          onChange={(v) => {
            setActive(Number(String(v).split('.')[0]) - 1)
            setSelected(null)
            setNewField(undefined)
          }}
          style={{ width: 200 }}
        />
        <InfoTip label="Mapping rules">
          Each rule checks the same input record. More than one rule can match. To produce a result, a rule needs a track key
          and any fields required by its kind. Samples shows which rules matched and which produced output.
        </InfoTip>
        <Button
          size="sm"
          variant="secondary"
          icon={<TbPlus />}
          onClick={() => {
            let number = rules.length + 1
            while (rules.some((r) => r.name === `rule ${number}`)) number++
            onChange({ ...mapping, rules: [...rules, { name: `rule ${number}`, key: '', fields: positionFields({}) }] })
            setActive(rules.length)
            setSelected(null)
            setNewField(undefined)
            setSettings(true)
            requestAnimationFrame(() => ruleSettings.current?.querySelector('input')?.focus())
          }}
        >
          Rule
        </Button>
        <span className="spacer" />
        {rule && (
          <>
            <Button size="sm" variant={settings ? 'primary' : 'secondary'} icon={<TbSettings />} onClick={() => setSettings(!settings)}>
              Rule settings
            </Button>
            <Button
              size="xs"
              variant="ghost"
              icon={<TbTrash />}
              aria-label="Remove rule"
              title="Remove rule"
              onClick={() => {
                onChange({ ...mapping, rules: rules.filter((_, j) => j !== i) })
                setActive(Math.max(0, i - 1))
                setSelected(null)
                setNewField(undefined)
                setEpoch(epoch + 1)
              }}
            />
          </>
        )}
      </div>

      {rule && settings && (
        <fieldset key={`settings:${i}:${epoch}`} className="map-outline" ref={ruleSettings}>
          <legend>Rule settings</legend>
          <div className="value-row">
            <Label size="sm">Name</Label>
            <InfoTip label="Rule name">Unique within the mapping; validation errors name the rule by it.</InfoTip>
            <Input style={{ ...INPUT, width: 180 }} aria-label="Rule name" value={String(rule.name ?? '')} onChange={(e) => setRule({ name: e.target.value })} />
            <Label size="sm">Kind</Label>
            <InfoTip label="Rule kind">
              observation: a report of an object; on a source with a Tracker stage, a detection for the tracker. static: identity fields (name, flag…)
              kept per track key and joined onto later observations with the same key, for as long as the identity join stage keeps them. track: a report of an
              object that carries its own identity (e.g. the sensor&apos;s own platform), which bypasses the Tracker stage. observation and track
              rules must map a position (latitude and longitude, or a polygon).
            </InfoTip>
            <FieldSelect
              ariaLabel="Rule kind"
              fields={KINDS.map((name) => ({ name }))}
              value={String(rule.kind ?? 'observation')}
              onChange={(v) => {
                if (!v) return
                const next = v === 'observation' ? drop(rule, 'kind') : { ...rule, kind: v }
                if (v !== 'static') next.fields = positionFields((rule.fields as Obj) ?? {})
                replaceRule(next)
              }}
              style={{ width: 130 }}
            />
          </div>
          <div className="value-row">
            <Label size="sm">Applies when</Label>
            <InfoTip label="Applies when">
              Choose a sample field and a test, then pick or enter a value. With no condition, the rule checks every record. Use Edit as JSON for advanced conditions.
            </InfoTip>
          </div>
          <ConditionBuilder label="Rule condition" value={rule.when} fields={sampleFields} verb="match" showSummary={false} onChange={(v) => (v === undefined ? replaceRule(drop(rule, 'when')) : setRule({ when: v }))} />
          <Disclosure label="Bindings">
            <InfoTip label="Bindings">
              Bindings give names to values that this rule can reuse. Start the names with an underscore to keep them separate from feed fields.
            </InfoTip>
            <JsonField label="Rule bindings" optional value={rule.let} placeholder="none" onChange={(v) => (v === undefined ? replaceRule(drop(rule, 'let')) : setRule({ let: v }))} />
          </Disclosure>
        </fieldset>
      )}

      {!rule ? (
        <span className="muted">No rules. Add one.</span>
      ) : (
        <div className="stack">
          <DataTable
            key={`${i}:${epoch}`}
            aria-label="Mapping"
            columns={columns}
            rows={rows}
            rowKey={(r) => r.id}
            selectedKey={selected}
            onRowClick={(r) => setSelected(r.id)}
            maxHeight={560}
            style={{ border: 0, borderRadius: 0 }}
          />
          <div className="value-row">
            <Button
              size="sm"
              variant="secondary"
              icon={<TbPlus />}
              disabled={newField !== undefined || fields.some(([k]) => k === '')}
              onClick={() => {
                setNewField('')
                setSelected('new-field')
              }}
            >
              Field
            </Button>
            <Button
              size="sm"
              variant="secondary"
              icon={<TbPlus />}
              onClick={() => {
                setRule({ identifiers: [...identifiers, { scheme: '', value: '' }] })
                setSelected(`id:${identifiers.length}`)
              }}
            >
              Identifier
            </Button>
            <InfoTip label="Mapping table">
              Source: the feed field a value comes from (a path such as <span className="mono">Message.PositionReport.Sog</span>); anything more is
              shown as a summary, edited by clicking the row. Destination: an OpenTrack field, an <span className="mono">ext.</span> field of the
              mapping&apos;s schema version, or <span className="mono">entity.&lt;key&gt;</span>, which updates the track&apos;s entity at a corroborated
              match. The track key is the feed&apos;s own id for the object; records without one are skipped. An identifier row adds an identifier
              under the scheme you name (mmsi, icao24, an ELNOT…), which the entity stage resolves. <TbAdjustments aria-hidden /> marks a value with
              transforms, a default or a lookup table.
            </InfoTip>
          </div>
          {row && (
            <fieldset className="map-outline">
              <legend>{row.kind === 'field' ? row.target || 'new field' : row.kind === 'key' ? 'track key' : `identifier ${row.target}`}</legend>
              <ValueEditor key={`${i}:${row.id}:${epoch}:${inlineRev}`} label={row.target} value={row.spec} onChange={(v) => setSpec(row, v)} />
            </fieldset>
          )}
        </div>
      )}
    </div>
  )
}
