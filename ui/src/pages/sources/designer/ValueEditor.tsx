import { useState } from 'react'
import { TbTable, TbX } from 'react-icons/tb'
import { Button, FieldSelect, Input } from '@kineticlogic/staresdk'
import type { ValueSpec } from '../../../lib/pipeline'
import { fromForm, INPUT, toForm, type Mode, type ValueForm } from '../../../lib/valueSpec'
import { InfoTip } from '../../../components/InfoTip'
import { JsonField } from './JsonField'

const MODE_HELP: Record<Mode, string> = {
  path: 'feed field: the value at this path in the record, e.g. Message.PositionReport.Sog or _frame.now.',
  first: 'first present of: the first of these fields (comma-separated) that is present and not null.',
  const: 'fixed value: the same value for every record. Text that reads as JSON (12, true, null) is taken as that.',
  template: 'template: text with {path} placeholders filled from the record, e.g. a-{aff}-A. Null if any placeholder is missing.',
  advanced: 'advanced (JSON): the full value expression, for cases, arithmetic, number ranges and keyed lookups.',
}

const MODES: { mode: Mode; name: string }[] = [
  { mode: 'path', name: 'feed field' },
  { mode: 'first', name: 'first present of' },
  { mode: 'const', name: 'fixed value' },
  { mode: 'template', name: 'template' },
  { mode: 'advanced', name: 'advanced (JSON)' },
]

/**
 * Where a value comes from and what is done to it, as a form: a feed field (or the first present
 * of several, a fixed value, a template), then transforms, values to ignore, a lookup table and a
 * default. Richer expressions are edited as JSON. Remount (change `key`) to load another value.
 */
export function ValueEditor({ label, value, onChange }: { label: string; value: ValueSpec | undefined; onChange: (v: ValueSpec) => void }) {
  const [form, setForm] = useState<ValueForm>(() => toForm(value))
  const [json, setJson] = useState<unknown>(value)

  const update = (patch: Partial<ValueForm>) => {
    const next = { ...form, ...patch }
    setForm(next)
    if (next.mode !== 'advanced') onChange(fromForm(next))
  }
  const switchMode = (name: string | null) => {
    const mode = MODES.find((m) => m.name === name)?.mode ?? 'path'
    if (mode === 'advanced') {
      const spec = form.mode === 'advanced' ? json : fromForm(form)
      setJson(spec)
      setForm({ ...form, mode })
      return
    }
    // Carry the source across: a path becomes the first of a list, and back.
    const seed = form.path || form.first.split(',')[0]?.trim() || ''
    update({ mode, path: form.path || seed, first: form.first || seed })
  }

  return (
    <div className="value-editor">
      <div className="value-row">
        <FieldSelect
          ariaLabel={`${label}: source`}
          fields={MODES.map((m) => ({ name: m.name }))}
          value={MODES.find((m) => m.mode === form.mode)!.name}
          onChange={switchMode}
          style={{ width: 150 }}
        />
        <InfoTip label={`${label}: source`}>
          {MODE_HELP[form.mode]}
          {form.mode !== 'const' && form.mode !== 'advanced' && ' Then, in order: values to ignore, transforms, the lookup table, and the default.'}
        </InfoTip>
        {form.mode === 'path' && (
          <Input style={INPUT} aria-label={`${label}: feed field`} placeholder="feed field" value={form.path} onChange={(e) => update({ path: e.target.value })} spellCheck={false} />
        )}
        {form.mode === 'first' && (
          <Input style={INPUT} aria-label={`${label}: fields to try`} placeholder="flight, r, hex" value={form.first} onChange={(e) => update({ first: e.target.value })} spellCheck={false} />
        )}
        {form.mode === 'const' && (
          <Input style={INPUT} aria-label={`${label}: value`} placeholder="value" value={form.constant} onChange={(e) => update({ constant: e.target.value })} spellCheck={false} />
        )}
        {form.mode === 'template' && (
          <Input style={INPUT} aria-label={`${label}: template`} placeholder="a-{aff}-A" value={form.template} onChange={(e) => update({ template: e.target.value })} spellCheck={false} />
        )}
      </div>
      {form.mode === 'advanced' ? (
        <JsonField
          label={`${label}: JSON`}
          value={json}
          onChange={(v) => {
            setJson(v)
            if (v !== undefined) onChange(v as ValueSpec)
          }}
        />
      ) : (
        form.mode !== 'const' && (
          <>
            <div className="value-row">
              <span className="muted">Transforms · ignore values · default</span>
              <InfoTip label={`${label}: transforms, ignore values, default`}>
                Transforms, comma-separated, applied in order: trim, upper, lower, nonempty (empty text becomes null), string, number, bool, time (RFC
                3339, common date forms or a Unix epoch), time_unix_s, time_unix_ms, knots_to_mps, feet_to_m, fpm_to_mps, kmh_to_mps, nm_to_m, wrap360
                (angle into 0–360), hex (hex text to a number), scale:x, offset:x, round:decimals, replace:from:to, split:separator:index (negative
                counts from the end), regex:pattern (first capture group), bits_any:mask (true if any of the bits is set).
                <br />
                Ignore values: comma-separated values the feed sends for &quot;not available&quot; (heading 511), treated as missing before the
                transforms.
                <br />
                Default: the value when all of the above gives nothing. Empty: the field is left out.
              </InfoTip>
            </div>
            <div className="value-row three">
              <Input
                style={INPUT}
                aria-label={`${label}: transforms`}
                placeholder="transforms"
                value={form.transforms}
                onChange={(e) => update({ transforms: e.target.value })}
                spellCheck={false}
              />
              <Input style={INPUT} aria-label={`${label}: ignore`} placeholder="ignore values" value={form.nullIf} onChange={(e) => update({ nullIf: e.target.value })} spellCheck={false} />
              <Input style={INPUT} aria-label={`${label}: default`} placeholder="default" value={form.defaultValue} onChange={(e) => update({ defaultValue: e.target.value })} spellCheck={false} />
            </div>
            {form.table ? (
              <div className="stack" style={{ gap: 4 }}>
                <div className="value-row">
                  <span className="muted">Lookup table · {Object.keys(form.table).length}</span>
                  <InfoTip label="Lookup table">
                    Replaces the value with the table&apos;s entry for it (matched as text), e.g. <span className="mono">{'{"B738": "civil"}'}</span>. Otherwise:
                    the value for anything not in the table; empty, it becomes null (then the default applies).
                  </InfoTip>
                  <span className="spacer" />
                  <Input style={{ ...INPUT, width: 180 }} aria-label={`${label}: table default`} placeholder="otherwise" value={form.tableDefault} onChange={(e) => update({ tableDefault: e.target.value })} spellCheck={false} />
                  <Button size="xs" variant="ghost" icon={<TbX />} aria-label="Remove lookup table" title="Remove lookup table" onClick={() => update({ table: null, tableDefault: '' })} />
                </div>
                <JsonField
                  label={`${label}: lookup table`}
                  value={form.table}
                  maxHeight={160}
                  onChange={(v) => v && typeof v === 'object' && !Array.isArray(v) && update({ table: v as Record<string, unknown> })}
                />
              </div>
            ) : (
              <div>
                <Button size="sm" variant="secondary" icon={<TbTable />} onClick={() => update({ table: {} })}>
                  Lookup table
                </Button>
              </div>
            )}
          </>
        )
      )}
    </div>
  )
}

