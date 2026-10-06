import { afterEach, describe, expect, it, vi } from 'vitest'
import { ApiError, api, CSRF_HEADER, withCsrf } from './client'

const reply = (status: number, body: unknown) =>
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify(body), { status })))

describe('status', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('keeps the dependency states when the server answers 503', async () => {
    reply(503, { service: 'opentrack', nats: { ok: false, error: 'not connected' } })
    const s = await api.status()
    expect(s.nats.ok).toBe(false)
    expect(s.nats.error).toBe('not connected')
  })

  it('throws on any other failure', async () => {
    reply(401, { error: 'sign in' })
    await expect(api.status()).rejects.toBeInstanceOf(ApiError)
    reply(503, { error: 'proxy says no' })
    await expect(api.status()).rejects.toThrow('proxy says no')
  })
})

describe('withCsrf', () => {
  it('adds the header the server checks, keeping the others', () => {
    const init = withCsrf('/api/v1/sources', { method: 'POST', headers: { 'content-type': 'application/json' } })
    const h = new Headers(init.headers)
    expect(h.get(CSRF_HEADER)).toBe('1')
    expect(h.get('content-type')).toBe('application/json')
    expect(init.method).toBe('POST')
  })
})
