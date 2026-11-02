import { describe, expect, it } from 'vitest'
import { DEFAULT_SYNC, type SyncSettings } from '../api/client'
import { acceptance, describeSync, directionBadge, syncWarnings } from './syncDirection'

const on = (patch: Partial<SyncSettings>): SyncSettings => ({ ...DEFAULT_SYNC, enabled: true, ...patch })
const keyed = (sites: string[]) => (s: string) => sites.includes(s)

describe('what a node does, in one line', () => {
  it('names what it sends and whom it takes in from', () => {
    expect(describeSync('AAA', DEFAULT_SYNC, keyed([]))).toBe('Sharing is off: AAA sends nothing and receives nothing.')
    expect(describeSync('AAA', on({ peers: ['BBB', 'CCC', 'DDD'] }), keyed(['BBB', 'DDD']))).toBe(
      'AAA sends its tracks and decisions to every node on the link and receives from BBB and DDD.',
    )
    expect(describeSync('AAA', on({ direction: 'receive', peers: ['BBB'] }), keyed(['BBB']))).toBe(
      'AAA receives tracks and decisions from BBB and sends nothing.',
    )
    expect(describeSync('AAA', on({ direction: 'receive', repair: true, peers: ['BBB'] }), keyed(['BBB']))).toContain(
      'sends only requests for what it missed',
    )
    expect(describeSync('AAA', on({ direction: 'receive', peers: ['BBB'] }), keyed([]))).toContain('from no node yet')
    expect(describeSync('AAA', on({ direction: 'send' }), keyed([]))).toBe(
      'AAA sends its tracks and decisions to every node on the link and takes in nothing.',
    )
    expect(describeSync('AAA', on({ direction: 'send', peers: ['BBB'] }), keyed(['BBB']))).toContain("answers BBB's requests")
  })

  it('shows a one-way node in the header', () => {
    expect(directionBadge(undefined)).toBeNull()
    expect(directionBadge(on({}))).toBeNull()
    expect(directionBadge({ enabled: false, direction: 'receive' })).toBeNull()
    expect(directionBadge(on({ direction: 'receive' }))).toBe('receive only')
    expect(directionBadge(on({ direction: 'send' }))).toBe('send only')
  })
})

describe('surprising combinations', () => {
  it('are warned about', () => {
    expect(syncWarnings(on({}))).toEqual([])
    expect(syncWarnings({ ...DEFAULT_SYNC, direction: 'receive' })).toEqual([])
    const r = syncWarnings(on({ direction: 'receive', share_profile: true }))
    expect(r).toHaveLength(2)
    expect(r[0]).toContain('changes made here stay on this node')
    expect(r[1]).toContain('Track management done here stays on this node')
    expect(syncWarnings(on({ direction: 'receive', share_profile: false, no_local_management: true }))).toEqual([])
    expect(syncWarnings(on({ direction: 'send', no_local_management: true }))[0]).toContain('no one can manage its tracks')
  })

  it('say whether a trusted node is taken in', () => {
    expect(acceptance(on({}), true)).toEqual({ ok: true, text: 'yes' })
    expect(acceptance(on({}), false).text).toBe('no key: refused')
    expect(acceptance(on({ direction: 'send' }), true).text).toBe('sends only')
    expect(acceptance(DEFAULT_SYNC, true).text).toBe('sharing off')
  })
})
