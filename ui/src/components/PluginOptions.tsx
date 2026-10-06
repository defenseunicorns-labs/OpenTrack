import { FieldSelect, Input, Toggle } from '@kineticlogic/staresdk'
import type { PluginInfo, PluginKind } from '../api/client'
import { INPUT } from '../lib/valueSpec'

type Obj = Record<string, unknown>

/** How a form lays out one labelled control. */
export type RowFn = (key: string, label: string, help: string | undefined, control: React.ReactNode) => React.ReactNode

/** A plugin picker for one kind: the enabled plugins that provide it. */
export function PluginPicker({
  plugins,
  kind,
  value,
  onChange,
  width = 240,
}: {
  plugins: PluginInfo[]
  kind: PluginKind
  value: string | undefined
  onChange: (p: PluginInfo) => void
  width?: number
}) {
  const usable = plugins.filter((p) => p.kinds.includes(kind) && p.enabled)
  const current = value && !usable.some((p) => p.name === value) ? [{ name: value, disabled: true, disabledReason: 'not loaded or disabled' }] : []
  return (
    <FieldSelect
      ariaLabel={`${kind} plugin`}
      fields={[...usable.map((p) => ({ name: p.name })), ...current]}
      value={value ?? null}
      onChange={(name) => {
        const p = usable.find((x) => x.name === name)
        if (p) onChange(p)
      }}
      style={{ width }}
    />
  )
}

/** The options a plugin's manifest describes, as form controls. */
export function PluginOptions({
  plugin,
  value,
  onChange,
  row,
}: {
  plugin: PluginInfo | undefined
  value: Obj
  onChange: (next: Obj) => void
  row: RowFn
}) {
  if (!plugin) return null
  const set = (k: string, v: unknown) => {
    const next = { ...value, [k]: v }
    if (v === undefined || v === '') delete next[k]
    onChange(next)
  }
  return (
    <>
      {plugin.options.map((o) => {
        const label = o.label || o.name
        let control: React.ReactNode
        if (o.type === 'bool') {
          control = <Toggle size="sm" aria-label={label} value={value[o.name] === true} onChange={(v) => set(o.name, v)} />
        } else if (o.type === 'choice') {
          control = (
            <FieldSelect
              ariaLabel={label}
              fields={o.choices.map((name) => ({ name }))}
              value={String(value[o.name] ?? o.default)}
              onChange={(v) => set(o.name, v ?? undefined)}
              style={{ width: 160 }}
            />
          )
        } else if (o.type === 'number') {
          control = (
            <span className="num-row">
              <Input
                style={{ ...INPUT, width: 120 }}
                type="number"
                aria-label={label}
                placeholder={o.default == null ? 'not set' : String(o.default)}
                value={value[o.name] == null ? '' : String(value[o.name])}
                onChange={(e) => set(o.name, e.target.value.trim() === '' ? undefined : Number(e.target.value))}
              />
              {o.unit && <span className="muted">{o.unit}</span>}
            </span>
          )
        } else {
          control = (
            <Input
              style={{ ...INPUT, width: 240 }}
              aria-label={label}
              placeholder={o.default ?? ''}
              value={String(value[o.name] ?? '')}
              onChange={(e) => set(o.name, e.target.value)}
              spellCheck={false}
            />
          )
        }
        return row(o.name, label, o.help || undefined, control)
      })}
    </>
  )
}
