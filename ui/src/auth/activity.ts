/**
 * When the user last did anything on the page (a key, a click, a scroll, a pointer move). Every
 * API call tells the server how long ago that was (`x-ot-idle-ms`), so the page's own polling does
 * not keep an unattended session alive: the server ends it after the idle timeout.
 */
let last = Date.now()

export const IDLE_HEADER = 'x-ot-idle-ms'

/** Milliseconds since the user last did anything. */
export const idleMs = () => Math.max(0, Date.now() - last)

export const markActive = () => {
  last = Date.now()
}

const EVENTS = ['pointerdown', 'keydown', 'wheel', 'touchstart', 'pointermove', 'scroll'] as const

/** Start noting activity; returns the cleanup. */
export function trackActivity(): () => void {
  let pending = false
  // Pointer moves and scrolls come in floods: note at most one a second.
  const note = () => {
    if (pending) return
    pending = true
    markActive()
    setTimeout(() => {
      pending = false
    }, 1000)
  }
  EVENTS.forEach((e) => window.addEventListener(e, note, { passive: true, capture: true }))
  return () => EVENTS.forEach((e) => window.removeEventListener(e, note, { capture: true }))
}

/** Init for a fetch with the idle header added (whatever form its headers take). */
export function withIdle(input: RequestInfo | URL, init: RequestInit | undefined): RequestInit {
  const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined))
  headers.set(IDLE_HEADER, String(idleMs()))
  return { ...init, headers }
}
