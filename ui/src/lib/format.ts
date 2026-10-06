import type { BadgeColor } from '@kineticlogic/staresdk'

export const fmtTime = (t: number | string | null | undefined) =>
  t === null || t === undefined || t === '' ? '—' : new Date(t).toISOString().replace('T', ' ').slice(0, 19) + 'Z'

export const fmtNum = (v: number | undefined | null, digits = 0, unit = '') =>
  v === undefined || v === null ? '—' : `${v.toFixed(digits)}${unit}`

export const fmtCount = (n: number | undefined) => (n === undefined ? '—' : n.toLocaleString())

/** Seconds since an ISO time, as "12s", "4m", "3h". */
export function ago(iso: string | undefined | null): string {
  if (!iso) return '—'
  const s = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000)
  if (s < 90) return `${Math.round(s)}s`
  if (s < 5400) return `${Math.round(s / 60)}m`
  return `${Math.round(s / 3600)}h`
}

export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/** A value as text: strings as they are, anything else as JSON. */
export const show = (v: unknown) => (typeof v === 'string' ? v : JSON.stringify(v))

/** Badge colour for a system track's state. */
export const STATE_COLOR: Record<string, BadgeColor> = {
  tentative: 'warning',
  confirmed: 'success',
  lost: 'grey',
  dropped: 'danger',
}
