import type { TakDelivery, TakOutput } from '../api/client'

export type TakKind = TakDelivery['kind']

export const TAK_KINDS: { kind: TakKind; label: string }[] = [
  { kind: 'tak_server', label: 'TAK Server' },
  { kind: 'multicast', label: 'Multicast' },
  { kind: 'listen', label: 'Listen for clients' },
]

export const kindLabel = (k: TakKind): string => TAK_KINDS.find((x) => x.kind === k)?.label ?? k

/** A new delivery of `kind`, with TAK's usual ports. */
export function newDelivery(kind: TakKind): TakDelivery {
  switch (kind) {
    case 'tak_server':
      return { kind, host: '', port: 8089, tls: { ca_file: '', cert_file: '', key_file: '' } }
    case 'multicast':
      return { kind, group: '239.2.3.1', port: 6969, ttl: 1 }
    case 'listen':
      return { kind, bind: '0.0.0.0:8089', tls: { cert_file: '', key_file: '' } }
  }
}

/** A new output with an id not yet taken. */
export function newOutput(taken: string[], kind: TakKind = 'tak_server'): TakOutput {
  let n = 1
  while (taken.includes(`tak-${n}`)) n++
  return { id: `tak-${n}`, enabled: true, stale_secs: 60, remarks: true, delivery: newDelivery(kind) }
}

/** Where an output sends, in a few characters. */
export function outputWhere(o: TakOutput): string {
  const d = o.delivery
  switch (d.kind) {
    case 'tak_server':
      return `${d.host || '?'}:${d.port}`
    case 'multicast':
      return `${isIpv6(d.group.trim()) ? `[${d.group.trim()}]` : d.group}:${d.port}${d.interface ? ` via ${d.interface}` : ''}`
    case 'listen':
      return d.bind
  }
}

export const isEncrypted = (d: TakDelivery): boolean => d.kind !== 'multicast' && d.tls != null

const IPV4 = /^(25[0-5]|2[0-4]\d|1?\d?\d)(\.(25[0-5]|2[0-4]\d|1?\d?\d)){3}$/

/** An IPv6 address (no zone), as the URL parser reads one. */
export function isIpv6(s: string): boolean {
  if (!s.includes(':') || s.includes('%')) return false
  try {
    new URL(`http://[${s}]/`)
    return true
  } catch {
    return false
  }
}

/** An interface name (eth0) or index (2), how an IPv6 group picks its interface. */
const IFACE = /^[!-~]{1,15}$/

/** What is wrong with an output, as the server would refuse it; null if nothing. */
export function outputProblem(o: TakOutput, others: TakOutput[]): string | null {
  if (!/^[A-Za-z0-9_-]{1,32}$/.test(o.id)) return 'The name is 1 to 32 letters, digits, - or _.'
  if (others.some((x) => x.id === o.id)) return `Another output is called ${o.id}.`
  if (!(o.stale_secs >= 10 && o.stale_secs <= 86400)) return 'Stale time is 10 to 86400 seconds.'
  const d = o.delivery
  const port = (p: number) => Number.isInteger(p) && p >= 1 && p <= 65535
  switch (d.kind) {
    case 'tak_server':
      if (!d.host.trim()) return "Give the TAK Server's host."
      if (!port(d.port)) return 'The port is 1 to 65535.'
      if (d.tls && !!d.tls.cert_file?.trim() !== !!d.tls.key_file?.trim()) return 'A client certificate needs its key, and a key its certificate.'
      return null
    case 'multicast':
    {
      const group = d.group.trim()
      const v6 = isIpv6(group)
      if (!v6 && !IPV4.test(group)) return 'The group is an IPv4 or IPv6 address, e.g. 239.2.3.1 or ff15::6969.'
      if (!port(d.port)) return 'The port is 1 to 65535.'
      if (!(Number.isInteger(d.ttl) && d.ttl >= 1 && d.ttl <= 255)) return 'TTL is 1 to 255.'
      const i = d.interface?.trim()
      if (!i) return null
      if (!v6) return IPV4.test(i) ? null : 'For an IPv4 group the interface is an IPv4 address of this host.'
      if (!/^ff/i.test(group)) return 'An IPv6 unicast address takes no interface.'
      if (IPV4.test(i) || i.includes(':') || i.includes('/') || !IFACE.test(i)) return 'For an IPv6 group the interface is a name (eth0) or index (2).'
      return null
    }
    case 'listen': {
      const m = /^(.+):(\d+)$/.exec(d.bind.trim())
      if (!m || !port(Number(m[2]))) return 'Listen on address:port, e.g. 0.0.0.0:8089.'
      if (d.tls && (!d.tls.cert_file.trim() || !d.tls.key_file.trim())) return 'TLS needs the server certificate and its key.'
      return null
    }
  }
}

/** The output as the server takes it: blank optional fields left out. */
export function cleanOutput(o: TakOutput): TakOutput {
  const blank = (s?: string) => (s?.trim() ? s.trim() : undefined)
  const d = o.delivery
  let delivery: TakDelivery
  switch (d.kind) {
    case 'tak_server':
      delivery = {
        kind: d.kind,
        host: d.host.trim(),
        port: d.port,
        ...(d.tls
          ? {
              tls: Object.fromEntries(
                Object.entries({ ca_file: blank(d.tls.ca_file), cert_file: blank(d.tls.cert_file), key_file: blank(d.tls.key_file), server_name: blank(d.tls.server_name) }).filter(
                  ([, v]) => v !== undefined,
                ),
              ),
            }
          : {}),
      }
      break
    case 'multicast':
      delivery = { kind: d.kind, group: d.group.trim(), port: d.port, ttl: d.ttl, ...(blank(d.interface) ? { interface: blank(d.interface) } : {}) }
      break
    case 'listen':
      delivery = {
        kind: d.kind,
        bind: d.bind.trim(),
        ...(d.tls
          ? {
              tls: {
                cert_file: d.tls.cert_file.trim(),
                key_file: d.tls.key_file.trim(),
                ...(blank(d.tls.client_ca_file) ? { client_ca_file: blank(d.tls.client_ca_file) } : {}),
                ...(d.tls.client_cert_optional && blank(d.tls.client_ca_file) ? { client_cert_optional: true } : {}),
                ...((d.tls.client_crl_files ?? []).filter((f) => f.trim()).length && blank(d.tls.client_ca_file)
                  ? { client_crl_files: (d.tls.client_crl_files ?? []).map((f) => f.trim()).filter(Boolean) }
                  : {}),
              },
            }
          : {}),
      }
      break
  }
  return { ...o, id: o.id.trim(), delivery }
}
