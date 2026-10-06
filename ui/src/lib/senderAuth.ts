import type { SourceSpec } from '../api/client'

const set = (v: unknown) => typeof v === 'string' && v.trim() !== ''

/**
 * Whether a listening transport authenticates its senders; null for one that connects out.
 * Mirrors the server's rule: mutual TLS with a required client certificate, or (gRPC) a bearer token.
 */
export function sendersAuthenticated(t: SourceSpec['transport']): boolean | null {
  const tls = (t.tls as Record<string, unknown> | undefined) ?? {}
  const mutual = set(tls.client_ca_file) && tls.client_cert_optional !== true
  switch (t.type) {
    case 'udp':
      return false
    case 'tcp_server':
      return mutual
    case 'grpc_server':
      return mutual || set(t.token)
    default:
      return null
  }
}

/** Whether the spec needs the risk acceptance it doesn't carry, for the wizard's "still needed" list. */
export function needsAcceptance(spec: SourceSpec): boolean {
  return sendersAuthenticated(spec.transport) === false && spec.unauthenticated !== 'accepted'
}
