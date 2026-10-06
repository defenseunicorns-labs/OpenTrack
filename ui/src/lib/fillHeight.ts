import { useCallback, useEffect, useRef, useState } from 'react'

/**
 * The height a filling DataTable (`.panel-body.fill > .ui-data-table`, `.panel-fill > …`) is
 * given by its panel, so it can virtualise against its real viewport instead of the 600 px
 * stareSDK assumes for `maxHeight="none"`. Put `ref` on the table's fill container and pass
 * `maxHeight={height ?? 'none'}` with `style={FILL_TABLE}` to the DataTable: the table's own box
 * never takes the measured cap (its flex share and `contain: size` size it), so there is no
 * resize feedback loop.
 */
export function useFillHeight(): [(el: HTMLElement | null) => void, number | undefined] {
  const [height, setHeight] = useState<number | undefined>(undefined)
  const observer = useRef<ResizeObserver | null>(null)
  const ref = useCallback((el: HTMLElement | null) => {
    observer.current?.disconnect()
    observer.current = null
    const table = el?.querySelector<HTMLElement>(':scope > .ui-data-table')
    if (!table || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => {
      const h = Math.ceil(table.getBoundingClientRect().height)
      if (h > 0) setHeight((old) => (old === h ? old : h))
    })
    ro.observe(table)
    observer.current = ro
  }, [])
  useEffect(() => () => observer.current?.disconnect(), [])
  return [ref, height]
}

/** A filling table is never capped by its own maxHeight (that number only sets its viewport). */
export const FILL_TABLE = { maxHeight: 'none' } as const
