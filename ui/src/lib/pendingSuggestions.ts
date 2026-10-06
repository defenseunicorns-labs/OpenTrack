import { useEffect, useState } from 'react'
import { api } from '../api/client'

/** Fired after a suggestion is accepted or rejected, so the count refreshes at once. */
export const SUGGESTIONS_CHANGED = 'ot:suggestions-changed'

const EVERY_MS = 15_000

/**
 * How many correlation suggestions wait for a track manager, polled every 15 s (and at once
 * after a decision). Zero while `enabled` is false (a viewer, who cannot act on them).
 */
export function usePendingSuggestions(enabled: boolean): number {
  const [open, setOpen] = useState(0)
  useEffect(() => {
    if (!enabled) {
      setOpen(0)
      return
    }
    let live = true
    const load = () =>
      api.suggestionCount().then(
        (r) => live && setOpen(r.open),
        () => {},
      )
    load()
    const t = setInterval(load, EVERY_MS)
    window.addEventListener(SUGGESTIONS_CHANGED, load)
    return () => {
      live = false
      clearInterval(t)
      window.removeEventListener(SUGGESTIONS_CHANGED, load)
    }
  }, [enabled])
  return open
}
