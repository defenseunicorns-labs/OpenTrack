import { TbPlus, TbTrash } from 'react-icons/tb'
import { Button, FieldSelect, Input } from '@kineticlogic/staresdk'
import type { ExtensionField, SchemaOverview } from '../../api/client'
import { validKey } from '../../lib/schemaUsage'
import { InfoTip } from '../../components/InfoTip'
import { FIELD_TYPES } from './schemaFields'

const CARD_OR_FEED = 'feed or entity'
// Inputs as tall as the selects beside them.
const ROW_INPUT = { height: 28, fontSize: 12 }

/** Why a field would be refused, checked as the user types (the server checks again on save). */
function problem(f: ExtensionField, all: ExtensionField[], reserved: string[]): string | null {
  const bad = validKey(f.key, reserved)
  if (bad === 'reserved') return 'This name is reserved. Choose another.'
  if (bad) return 'Start the name with a lowercase letter. Use up to 64 lowercase letters, numbers, or underscores.'
  if (all.filter((x) => x.key === f.key).length > 1) return 'This name is already in the draft.'
  if (f.type === 'enum' && !(f.enum_values ?? []).length) return 'Enter at least one allowed value.'
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
            Start with a lowercase letter. Use up to 64 lowercase letters, numbers, or underscores. The names registry and entity are reserved.
            Feed mappings use <code>ext.&lt;name&gt;</code>. Other systems receive it as <code>attributes.&lt;name&gt;</code>.
          </InfoTip>
        </span>
        <span>
          Type
          <InfoTip label="Type">
            Choose the kind of value this field sends. For example, string is text, integer is a whole number, and enum lets you pick the allowed values.
            A timestamp uses RFC 3339, such as 2026-01-01T12:00:00Z. A position needs latitude and longitude in degrees. JSON keeps the value as it is.
            OpenTrack skips values it cannot convert to the selected type.
          </InfoTip>
        </span>
        <span>
          Unit
          <InfoTip label="Unit">Enter the unit the value already uses, such as meters or knots. It tells readers what the number means. OpenTrack leaves the number as it is.</InfoTip>
        </span>
        <span>
          Filled by
          <InfoTip label="Filled by">
            Choose feed or entity to use a feed mapping or a linked registry value. If both provide a value, the registry value wins.
            Choose a built-in, such as speed_mps, to let OpenTrack fill the value and set its type.
          </InfoTip>
        </span>
        <span>Notes<InfoTip label="Notes">Describe what the field means for people who maintain the schema or use its data.</InfoTip></span>
        <span />
      </div>
      {fields.map((f, i) => {
        const err = problem(f, fields, schema.reserved_extension_keys)
        const feeds = usage[f.key] ?? []
        const builtin = builtins.find((b) => b.name === f.builtin)
        return (
          <div key={i} className="field-block">
            <div className="field-row">
              <Input style={ROW_INPUT} aria-label={`Field ${i + 1} name`} value={f.key} onChange={(e) => set(i, { key: e.target.value })} autoComplete="off" spellCheck={false} />
              {f.builtin ? (
                <span className="mono muted">
                  {f.type}
                  <InfoTip label="Built-in type">
                    OpenTrack chooses the type for {f.builtin}. Select feed or entity under Filled by to choose a type yourself.
                    {builtin?.reads?.length ? ` OpenTrack reads this value from ${builtin.reads.join(', ')}.` : ''}
                  </InfoTip>
                </span>
              ) : (
                <FieldSelect
                  ariaLabel={`Field ${i + 1} type`}
                  fields={FIELD_TYPES.map((name) => ({ name }))}
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
              <Input style={ROW_INPUT} aria-label={`Field ${i + 1} notes`} value={f.description ?? ''} onChange={(e) => set(i, { description: e.target.value || undefined })} />
              <div className="schema-field-actions">
                {!f.builtin && feeds.length > 0 && (
                  <InfoTip label={`Feeds for ${f.key}`}>
                    These feeds supply this field: {feeds.join(', ')}. A linked registry value takes priority.
                  </InfoTip>
                )}
                <Button size="xs" variant="ghost" icon={<TbTrash />} aria-label={`Remove ${f.key || 'field'}`} title="Remove field" onClick={() => onChange(fields.filter((_, j) => j !== i))} />
              </div>
            </div>
            {f.type === 'enum' && (
              <div className="field-sub schema-enum-values">
                <Input
                  style={ROW_INPUT}
                  aria-label={`Field ${i + 1} values`}
                  value={(f.enum_values ?? []).join(', ')}
                  placeholder="Allowed values"
                  onChange={(e) => set(i, { enum_values: e.target.value.split(',').map((v) => v.trim()).filter(Boolean) })}
                />
                <InfoTip label="Allowed values">Separate each choice with a comma. For example, low, medium, high.</InfoTip>
              </div>
            )}
            {err && <div className="field-sub error-text">{err}</div>}
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
