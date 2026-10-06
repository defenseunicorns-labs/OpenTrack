import { describe, expect, it } from 'vitest'
import type { SourceSpec } from '../api/client'
import { needsAcceptance, sendersAuthenticated } from './senderAuth'

const spec = (transport: SourceSpec['transport'], accepted = false): SourceSpec => ({
  id: 's',
  name: 'S',
  transport,
  pipeline: { codec: { type: 'json' }, mapping: { rules: [] } },
  ...(accepted ? { unauthenticated: 'accepted' as const } : {}),
})

describe('sender authentication', () => {
  it('matches the server: UDP never, TCP with required client certificates, gRPC with those or a token', () => {
    expect(sendersAuthenticated({ type: 'udp', bind: '0.0.0.0:6969' })).toBe(false)
    expect(sendersAuthenticated({ type: 'tcp_server', bind: ':1', tls: { cert_file: 'a', key_file: 'b' } })).toBe(false)
    const mutual = { cert_file: 'a', key_file: 'b', client_ca_file: 'ca.pem' }
    expect(sendersAuthenticated({ type: 'tcp_server', bind: ':1', tls: mutual })).toBe(true)
    expect(sendersAuthenticated({ type: 'tcp_server', bind: ':1', tls: { ...mutual, client_cert_optional: true } })).toBe(false)
    expect(sendersAuthenticated({ type: 'grpc_server', bind: ':1', token: '${env:T}' })).toBe(true)
    expect(sendersAuthenticated({ type: 'grpc_server', bind: ':1' })).toBe(false)
    expect(sendersAuthenticated({ type: 'tcp_client', host: 'h', port: 1 })).toBeNull()
  })

  it('asks for the acceptance only where it is missing and needed', () => {
    expect(needsAcceptance(spec({ type: 'udp', bind: ':1' }))).toBe(true)
    expect(needsAcceptance(spec({ type: 'udp', bind: ':1' }, true))).toBe(false)
    expect(needsAcceptance(spec({ type: 'http_poll', url: 'https://x' }))).toBe(false)
  })
})
