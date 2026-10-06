import { describe, expect, it } from 'vitest'
import { IDLE_HEADER, idleMs, markActive, withIdle } from './activity'

describe('activity', () => {
  it('adds how long the user has been idle to a request', () => {
    markActive()
    expect(idleMs()).toBeLessThan(1000)
    const init = withIdle('/api/v1/status', { headers: { 'content-type': 'application/json' } })
    const h = new Headers(init.headers)
    expect(h.get('content-type')).toBe('application/json')
    expect(Number(h.get(IDLE_HEADER))).toBeGreaterThanOrEqual(0)
  })
})
