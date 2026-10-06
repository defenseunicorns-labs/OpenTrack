import { useEffect, useMemo, useState } from 'react'
import { TbBraces, TbListDetails, TbPlus, TbX } from 'react-icons/tb'
import { Button, Input, Select } from '@kineticlogic/staresdk'
import { describeCondition } from '../../../lib/pipeline'
import { fromBuilt, OPS, takes, toBuilt, type Built, type Op, type Rule, type SampleField } from '../../../lib/conditions'
import { JsonField } from './JsonField'

const OTHER = '\u0000other'
const MATCH = [
  { value: 'all', label: 'all of these' },
  { value: 'any', label: 'any of these' },
]
const OP_OPTIONS = OPS.map((o) => ({ value: o.value, label: o.label }))

const PLACEHOLDER: Record<string, string> = {
  one: 'value',
  list: 'values, separated by commas',
  number: 'number',
  text: 'text',
}

/** A short example of a sample value, for the field list. */
function example(v: unknown): string {
  const s = typeof v === 'string' ? v : JSON.stringify(v)
  return s.length > 24 ? `${s.slice(0, 24)}…` : s
}

/**
 * A condition built from rows of "field, test, value", matched all or any. The fields come from the
 * stored samples as they reach this stage, with any other path typed in. A condition the rows cannot
 * show (nested groups, transformed values) opens in the JSON editor, which is a toggle away anyway.
 */
export function ConditionBuilder({
  label,
  value,
  onChange,
  fields,
  verb,
}: {
  label: string
  value: unknown
  onChange: (v: unknown) => void
  fields: SampleField[]
  /** How the summary reads: "keep" or "drop". */
  verb: string
}) {
  const kinds = useMemo(() => Object.fromEntries(fields.map((f) => [f.path, f.kind])), [fields])
  const [built, setBuilt] = useState<Built | null>(() => toBuilt(value))
  const [json, setJson] = useState(() => toBuilt(value) === null)
  // Take up a value changed elsewhere (the JSON editor, a reset) unless it is what the rows make.
  useEffect(() => {
    const ours = built ? fromBuilt(built, kinds) : undefined
    if (JSON.stringify(ours) === JSON.stringify(value ?? undefined)) return
    const next = toBuilt(value)
    setBuilt(next)
    if (next === null) setJson(true)
    // eslint-disable-next-line react-hooks/exhaustive-deps -- only an outside change re-reads the rows
  }, [value])

  const update = (next: Built) => {
    setBuilt(next)
    onChange(fromBuilt(next, kinds))
  }
  const setRule = (i: number, patch: Partial<Rule>) => built && update({ ...built, rules: built.rules.map((r, j) => (j === i ? { ...r, ...patch } : r)) })
  const known = new Set(fields.map((f) => f.path))
  const fieldOptions = [...fields.map((f) => ({ value: f.path, label: `${f.path}  (e.g. ${example(f.example)})` })), { value: OTHER, label: 'Other path…' }]
  const [typing, setTyping] = useState<Set<number>>(new Set())

  const summary = value === undefined ? 'nothing yet' : `${verb} when ${describeCondition(value)}`
  const toggle = (
    <Button
      size="sm"
      variant="ghost"
      icon={json ? <TbListDetails /> : <TbBraces />}
      disabled={json && built === null}
      title={json && built === null ? 'This condition needs JSON: it has nested groups or transformed values.' : undefined}
      onClick={() => setJson(!json)}
    >
      {json ? 'Edit as rows' : 'Edit as JSON'}
    </Button>
  )

  if (json || built === null) {
    return (
      <div className="stack" style={{ gap: 4 }}>
        <JsonField label={label} optional value={value} onChange={onChange} describe={(v) => `${verb} when ${describeCondition(v)}`} />
        <div className="toolbar">{toggle}</div>
      </div>
    )
  }

  return (
    <div className="stack" style={{ gap: 4 }}>
      {built.rules.length > 1 && (
        <div className="condition-match">
          <span className="muted">Match</span>
          <Select ariaLabel={`${label}: match`} options={MATCH} value={built.match} onChange={(m) => update({ ...built, match: m === 'any' ? 'any' : 'all' })} style={{ width: 140 }} />
        </div>
      )}
      {built.rules.map((r, i) => {
        const free = typing.has(i) || (r.path !== '' && !known.has(r.path))
        const need = takes(r.op)
        return (
          <div className="condition-row" key={i}>
            {free ? (
              <Input
                aria-label={`${label}: field ${i + 1}`}
                placeholder="path, e.g. source_track_key"
                value={r.path}
                onChange={(e) => setRule(i, { path: e.target.value })}
                autoComplete="off"
                spellCheck={false}
              />
            ) : (
              <Select
                ariaLabel={`${label}: field ${i + 1}`}
                placeholder="Choose a field"
                options={fieldOptions}
                value={r.path || null}
                onChange={(p) => {
                  if (p === OTHER) setTyping(new Set(typing).add(i))
                  else setRule(i, { path: p ?? '' })
                }}
              />
            )}
            <Select ariaLabel={`${label}: test ${i + 1}`} options={OP_OPTIONS} value={r.op} onChange={(op) => op && setRule(i, { op: op as Op })} />
            {need === 'none' ? (
              <span />
            ) : (
              <Input
                aria-label={`${label}: value ${i + 1}`}
                placeholder={PLACEHOLDER[need]}
                inputMode={need === 'number' ? 'decimal' : undefined}
                value={r.value}
                onChange={(e) => setRule(i, { value: e.target.value })}
                autoComplete="off"
                spellCheck={false}
              />
            )}
            <Button
              size="xs"
              variant="ghost"
              icon={<TbX />}
              aria-label={`Remove test ${i + 1}`}
              onClick={() => {
                setTyping(new Set([...typing].filter((t) => t !== i).map((t) => (t > i ? t - 1 : t))))
                update({ ...built, rules: built.rules.filter((_, j) => j !== i) })
              }}
            />
          </div>
        )
      })}
      <div className="toolbar">
        <Button size="sm" variant="ghost" icon={<TbPlus />} onClick={() => setBuilt({ ...built, rules: [...built.rules, { path: '', op: 'eq', value: '' }] })}>
          {built.rules.length ? 'Add a test' : 'Add a condition'}
        </Button>
        {toggle}
      </div>
      <span className="muted">{summary}</span>
    </div>
  )
}
