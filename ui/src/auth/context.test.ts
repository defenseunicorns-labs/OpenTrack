import { describe, expect, it } from 'vitest'
import { canHere } from './context'

describe('canHere', () => {
  it('goes by the role', () => {
    expect(canHere({ role: 'admin' }, 'admin')).toBe(true)
    expect(canHere({ role: 'track_manager' }, 'admin')).toBe(false)
    expect(canHere({ role: 'track_manager' }, 'viewer')).toBe(true)
    expect(canHere(null, 'viewer')).toBe(false)
  })

  it('hides admin work where the admin routes are not served', () => {
    expect(canHere({ role: 'admin', admin_api: false }, 'admin')).toBe(false)
    expect(canHere({ role: 'admin', admin_api: false }, 'track_manager')).toBe(true)
    expect(canHere({ role: 'admin', admin_api: true }, 'admin')).toBe(true)
  })
})
