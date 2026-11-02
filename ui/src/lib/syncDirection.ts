import type { SyncDirection, SyncSettings } from '../api/client'

/** The names of the directions, as Settings → Nodes offers them. */
export const DIRECTIONS: { value: SyncDirection; label: string }[] = [
  { value: 'both', label: 'Send and receive' },
  { value: 'receive', label: 'Receive only' },
  { value: 'send', label: 'Send only' },
]

/** The short form for the header badge: none when the node sends and receives (or does not sync). */
export function directionBadge(s: Pick<SyncSettings, 'enabled' | 'direction'> | undefined): string | null {
  if (!s?.enabled) return null
  if (s.direction === 'receive') return 'receive only'
  if (s.direction === 'send') return 'send only'
  return null
}

function list(sites: string[]): string {
  if (sites.length <= 1) return sites.join('')
  return `${sites.slice(0, -1).join(', ')} and ${sites[sites.length - 1]}`
}

/**
 * What this node does, in one sentence: what it sends, and whose tracks and decisions it takes in (trusted nodes with
 * a pinned key; any other node's messages are refused).
 */
export function describeSync(site: string, s: SyncSettings, keyed: (site: string) => boolean): string {
  if (!s.enabled) return `Sharing is off: ${site} sends nothing and receives nothing.`
  const from = s.peers.filter(keyed)
  const fromText = from.length > 0 ? list(from) : 'no node yet (trust a node and pin its key)'
  switch (s.direction) {
    case 'receive':
      return s.repair
        ? `${site} receives tracks and decisions from ${fromText}, and sends only requests for what it missed: no tracks, no decisions.`
        : `${site} receives tracks and decisions from ${fromText} and sends nothing.`
    case 'send':
      return from.length > 0
        ? `${site} sends its tracks and decisions to every node on the link and takes in nothing; it answers ${list(from)}'s requests for what they missed.`
        : `${site} sends its tracks and decisions to every node on the link and takes in nothing.`
    default:
      return `${site} sends its tracks and decisions to every node on the link and receives from ${fromText}.`
  }
}

/** Combinations of settings that do something people may not expect. */
export function syncWarnings(s: SyncSettings): string[] {
  if (!s.enabled) return []
  const out: string[] = []
  if (s.direction === 'receive') {
    if (s.share_profile)
      out.push(
        'Share the profile is on: other nodes’ output schema and correlation settings apply here, but changes made here stay on this node.',
      )
    if (!s.no_local_management) out.push('Track management done here stays on this node: other nodes never see it.')
  }
  if (s.direction === 'send' && s.no_local_management)
    out.push('This node takes in nothing and accepts no track management here, so no one can manage its tracks.')
  return out
}

/** Whether this node takes in a trusted node's tracks and decisions, and why not. */
export function acceptance(s: SyncSettings, hasKey: boolean): { ok: boolean; text: string } {
  if (!s.enabled) return { ok: false, text: 'sharing off' }
  if (!hasKey) return { ok: false, text: 'no key: refused' }
  if (s.direction === 'send') return { ok: false, text: 'sends only' }
  return { ok: true, text: 'yes' }
}
