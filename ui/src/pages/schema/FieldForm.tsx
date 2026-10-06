import { TbPlus, TbTrash } from 'react-icons/tb'
import { Button, FieldSelect, Input } from '@kineticlogic/staresdk'
import type { ExtensionField, SchemaOverview } from '../../api/client'
import { validKey } from '../../lib/schemaUsage'
import { InfoTip } from '../../components/InfoTip'

const TYPES: ExtensionField['type'][] = ['string', 'integer', 'number', 'boolean', 'enum', 'timestamp', 'position', 'json']
const CARD_OR_FEED = 'feed or entity'
// Inputs as tall as the selects beside them.
const ROW_INPUT = { height: 28, fontSize: 12 }

/** Why a field would be refused, checked as the user types (the server checks again on save). */
function problem(f: ExtensionField, all: ExtensionField[], reserved: string[]): string | null {
  const bad = validKey(f.key, reserved)
  if (bad) return `name: ${bad}`
  if (all.filter((x) => x.key === f.key).length > 1) return 'name: used twice'
  if (f.type === 'enum' && !(f.enum_values ?? []).length) return 'enum: list at least one value'
  return null
}

/**
 * The draft's fields as a form: one row per field with its name, type, unit, where its value comes
 * from (a feed mapping or an entity link, or an OpenTrack built-in, which fixes the type) and notes.
 */
export function FieldForm({
  fields,
  onChange,
  schema,
  usage,
}: {
  fields: ExtensionField[]
  onChange: (f: ExtensionField[]) => void
  schema: SchemaOverview
  /** Source ids feeding each field, by key. */
  usage: Record<string, string[]>
}) {
  const builtins = schema.builtins ?? []
  const set = (i: number, patch: Partial<ExtensionField>) => {
    const next = fields.map((f, j) => (j === i ? { ...f, ...patch } : f))
    // Drop keys set to undefined so the saved JSON stays clean.
    onChange(
      next.map(
        (f) =>
          Object.fromEntries(
            Object.entries(f).filter(([k, v]) => k === 'key' || k === 'type' || (v !== undefined && v !== '')),
          ) as unknown as ExtensionField,
      ),
    )
  }
  return (
    <div className="field-form">
      <div className="field-row head">
        <span>
          Name
          <InfoTip label="Name">
            Published as <span className="mono">attributes.&lt;name&gt;</span> and mapped as <span className="mono">ext.&lt;name&gt;</span>: a
            lower-case letter, then letters, digits or _, at most 64. registry and entity are reserved.
          </InfoTip>
        </span>
        <span>
          Type
          <InfoTip label="Type">
            What values are converted to before publishing: string, integer, number, boolean (true/false, yes/no, 1/0), enum (one of a list
            of values), timestamp (RFC 3339), position (an object with latitude and longitude in degrees) or json (any value, kept as is). A
            feed value that does not convert is dropped.
          </InfoTip>
        </span>
        <span>
          Unit
          <InfoTip label="Unit">A label for people reading the schema (m, kn, deg…). OpenTrack does not convert values.</InfoTip>
        </span>
        <span>
          Filled by
          <InfoTip label="Filled by">
            {CARD_OR_FEED}: a source mapping sets <span className="mono">ext.&lt;name&gt;</span>, or a pipeline links an entity attribute to it
            (the entity&apos;s value wins). A built-in fills it from OpenTrack instead: state, cot_type, course_deg, speed_mps, heading_deg,
            vertical_rate_mps, altitude_hae_m, cep_m, callsign, identifiers, platform_type/flag/hull, sensor_code, confidence, sources,
            contributors, observation_count, first_seen, last_seen or entity_id. A built-in fixes the type.
          </InfoTip>
        </span>
        <span>Notes</span>
        <span />
      </div>
      {fields.map((f, i) => {
        const err = problem(f, fields, schema.reserved_extension_keys)
        const feeds = usage[f.key] ?? []
        return (
          <div key={i} className="field-block">
            <div className="field-row">
              <Input style={ROW_INPUT} aria-label={`Field ${i + 1} name`} value={f.key} onChange={(e) => set(i, { key: e.target.value })} autoComplete="off" spellCheck={false} />
              {f.builtin ? (
                <span className="mono muted">
                  {f.type}
                  <InfoTip label="Built-in type">Fixed by the built-in {f.builtin}; pick feed or entity under Filled by to change it.</InfoTip>
                </span>
              ) : (
                <FieldSelect
                  ariaLabel={`Field ${i + 1} type`}
                  fields={TYPES.map((name) => ({ name }))}
                  value={f.type}
                  onChange={(v) => set(i, { type: (v ?? 'string') as ExtensionField['type'], enum_values: v === 'enum' ? f.enum_values : undefined })}
                />
              )}
              <Input style={ROW_INPUT} aria-label={`Field ${i + 1} unit`} value={f.unit ?? ''} placeholder="—" onChange={(e) => set(i, { unit: e.target.value || undefined })} />
              <FieldSelect
                ariaLabel={`Field ${i + 1} filled by`}
                fields={[{ name: CARD_OR_FEED }, ...builtins.map((b) => ({ name: b.name }))]}
                value={f.builtin ?? CARD_OR_FEED}
                onChange={(v) => {
                  const b = builtins.find((x) => x.name === v)
                  set(i, b ? { builtin: b.name, type: b.type, enum_values: undefined } : { builtin: undefined })
                }}
              />
              <Input style={ROW_INPUT} aria-label={`Field ${i + 1} notes`} value={f.description ?? ''} placeholder="What it is, for maintainers and consumers" onChange={(e) => set(i, { description: e.target.value || undefined })} />
              <Button size="xs" variant="ghost" icon={<TbTrash />} aria-label={`Remove ${f.key || 'field'}`} title="Remove field" onClick={() => onChange(fields.filter((_, j) => j !== i))} />
            </div>
            {f.type === 'enum' && (
              <div className="field-sub">
                <Input
                  style={ROW_INPUT}
                  aria-label={`Field ${i + 1} values`}
                  value={(f.enum_values ?? []).join(', ')}
                  placeholder="Allowed values, comma-separated"
                  onChange={(e) => set(i, { enum_values: e.target.value.split(',').map((v) => v.trim()).filter(Boolean) })}
                />
              </div>
            )}
            {(err || f.builtin || feeds.length > 0) && (
              <div className="field-sub muted">
                {err ? (
                  <span className="error-text">{err}</span>
                ) : f.builtin ? (
                  `OpenTrack fills it${builtins.find((b) => b.name === f.builtin)?.reads?.length ? ` from ${builtins.find((b) => b.name === f.builtin)!.reads!.join(', ')}` : ''}; feeds and entities do not.`
                ) : (
                  `Fed by ${feeds.join(', ')}; where a pipeline links it to an entity attribute, the entity's value takes precedence.`
                )}
              </div>
            )}
          </div>
        )
      })}
      <div>
        <Button size="sm" variant="secondary" icon={<TbPlus />} onClick={() => onChange([...fields, { key: '', type: 'string' }])}>
          Add field
        </Button>
      </div>
    </div>
  )
}
