import { useState } from 'react'
import { readPanelState } from '@kineticlogic/staresdk'

/**
 * A CollapsiblePanel's open state, kept by the page (controlled mode) so the page can size the
 * panel by it, and remembered under the same key the panel's own `persistKey` uses.
 */
export function usePanelOpen(key: string, fallback = true): [boolean, (open: boolean) => void] {
  const [open, setOpen] = useState(() => readPanelState(key, fallback))
  const set = (next: boolean) => {
    setOpen(next)
    try {
      localStorage.setItem(key, String(next))
    } catch {
      /* ignore */
    }
  }
  return [open, set]
}

/** An open panel that takes the page height left over; it never gets shorter than a useful table. */
export const FILL_PANEL = { flex: '1 1 0', minHeight: 280 } as const
