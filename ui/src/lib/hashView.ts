import { useCallback, useEffect, useState } from 'react'

/** The part of `location.hash` before any `/`, e.g. `#sources/ais` → `sources`. */
function read(fallback: string): [string, string] {
  const h = window.location.hash.replace(/^#/, '')
  const [view, ...rest] = h.split('/')
  return [view || fallback, decodeURIComponent(rest.join('/'))]
}

/**
 * The current view and sub-path in the URL hash (`#<view>/<sub>`), so a reload or a shared
 * link lands on the same screen.
 */
export function useHashView(fallback: string) {
  const [state, setState] = useState(() => read(fallback))
  useEffect(() => {
    const onHash = () => setState(read(fallback))
    window.addEventListener('hashchange', onHash)
    return () => window.removeEventListener('hashchange', onHash)
  }, [fallback])
  const go = useCallback((view: string, sub = '') => {
    window.location.hash = sub ? `${view}/${encodeURIComponent(sub)}` : view
  }, [])
  return { view: state[0], sub: state[1], go }
}
