import { describe, expect, it } from 'vitest'
import type { TakOutput } from '../api/client'
import { cleanOutput, isEncrypted, newDelivery, newOutput, outputProblem, outputWhere } from './tak'

describe('TAK outputs', () => {
  it('starts new outputs with free names and TAK defaults', () => {
    const o = newOutput(['tak-1'], 'multicast')
    expect(o.id).toBe('tak-2')
    expect(o.delivery).toEqual({ kind: 'multicast', group: '239.2.3.1', port: 6969, ttl: 1 })
    expect(isEncrypted(o.delivery)).toBe(false)
    expect(isEncrypted(newDelivery('tak_server'))).toBe(true)
    expect(outputWhere(o)).toBe('239.2.3.1:6969')
  })

  it('finds what the server would refuse', () => {
    const o = newOutput([], 'tak_server')
    expect(outputProblem(o, [])).toMatch(/host/)
    const ok: TakOutput = { ...o, delivery: { kind: 'tak_server', host: 'tak.example', port: 8089, tls: { cert_file: 'c.pem', key_file: '' } } }
    expect(outputProblem(ok, [])).toMatch(/key/)
    ok.delivery = { kind: 'tak_server', host: 'tak.example', port: 8087 }
    expect(outputProblem(ok, [])).toBeNull()
    expect(outputProblem(ok, [ok])).toMatch(/Another/)
    expect(outputProblem({ ...ok, id: 'a b' }, [])).toMatch(/letters/)
    expect(outputProblem({ ...ok, stale_secs: 5 }, [])).toMatch(/Stale/)
    expect(outputProblem({ ...ok, delivery: { kind: 'multicast', group: '239.2.3.1', port: 6969, ttl: 0 } }, [])).toMatch(/TTL/)
    expect(outputProblem({ ...ok, delivery: { kind: 'multicast', group: 'x', port: 6969, ttl: 1 } }, [])).toMatch(/group/)
    const mc = (group: string, iface?: string) => outputProblem({ ...ok, delivery: { kind: 'multicast', group, port: 6969, ttl: 1, interface: iface } }, [])
    expect(mc('239.2.3.1', '192.168.1.20')).toBeNull()
    expect(mc('239.2.3.1', 'eth0')).toMatch(/IPv4 address/)
    expect(mc('ff15::6969')).toBeNull()
    expect(mc('ff15::6969', 'eth0')).toBeNull()
    expect(mc('ff15::6969', '2')).toBeNull()
    expect(mc('ff15::6969', '192.168.1.20')).toMatch(/name/)
    expect(mc('ff15::6969', 'fe80::1')).toMatch(/name/)
    expect(mc('2001:db8::1')).toBeNull()
    expect(mc('2001:db8::1', 'eth0')).toMatch(/unicast/)
    expect(mc('ff15::zz')).toMatch(/group/)
    expect(outputProblem({ ...ok, delivery: { kind: 'listen', bind: '8089' } }, [])).toMatch(/address:port/)
    expect(outputProblem({ ...ok, delivery: { kind: 'listen', bind: '0.0.0.0:8089', tls: { cert_file: 'c', key_file: '' } } }, [])).toMatch(/certificate/)
  })

  it('leaves blank optional fields out', () => {
    const o = cleanOutput({
      id: ' eud ',
      enabled: true,
      stale_secs: 60,
      remarks: true,
      delivery: { kind: 'listen', bind: ' 0.0.0.0:8089 ', tls: { cert_file: 'c', key_file: 'k', client_ca_file: ' ', client_cert_optional: true, client_crl_files: [''] } },
    })
    expect(o.id).toBe('eud')
    expect(o.delivery).toEqual({ kind: 'listen', bind: '0.0.0.0:8089', tls: { cert_file: 'c', key_file: 'k' } })
    const t = cleanOutput({ ...o, delivery: { kind: 'tak_server', host: 'h', port: 8089, tls: { ca_file: 'ca', cert_file: '', key_file: '' } } })
    expect(t.delivery).toEqual({ kind: 'tak_server', host: 'h', port: 8089, tls: { ca_file: 'ca' } })
    const m = cleanOutput({ ...o, delivery: { kind: 'multicast', group: '239.2.3.1', port: 6969, ttl: 1, interface: '' } })
    expect(m.delivery).toEqual({ kind: 'multicast', group: '239.2.3.1', port: 6969, ttl: 1 })
  })
})
