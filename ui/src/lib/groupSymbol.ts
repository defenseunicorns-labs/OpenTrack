
/**
 * A group's MIL-STD-2525C symbol code, built from its parts:
 *
 *   S {affiliation} {dimension} P {function ×6} {HQ / task force} {echelon} -- -
 *
 * The function is a naval task organisation icon (Navy Task Force S*SPGT, Task Group GG, Task Unit
 * GU, Task Element GE, Convoy GC) or the function the members share (a flight of bombers keeps the
 * bomber icon). Position 11 is `E` for a task force (milsymbol draws the bracket), position 12 the
 * echelon amplifier (A team/crew … N command).
 */

export const AFFILIATION_LETTER: Record<string, string> = {
  pending: 'P',
  unknown: 'U',
  assumed_friend: 'A',
  friend: 'F',
  neutral: 'N',
  suspect: 'S',
  hostile: 'H',
  joker: 'J',
  faker: 'K',
  none: 'O',
}

export const DIMENSION_LETTER: Record<string, string> = { air: 'A', ground: 'G', surface: 'S', subsurface: 'U', space: 'P' }

export const ECHELONS: { letter: string; name: string }[] = [
  { letter: 'A', name: 'Team / crew' },
  { letter: 'B', name: 'Squad' },
  { letter: 'C', name: 'Section' },
  { letter: 'D', name: 'Platoon / detachment' },
  { letter: 'E', name: 'Company / battery / troop' },
  { letter: 'F', name: 'Battalion / squadron' },
  { letter: 'G', name: 'Regiment / group' },
  { letter: 'H', name: 'Brigade' },
  { letter: 'I', name: 'Division' },
  { letter: 'J', name: 'Corps' },
  { letter: 'K', name: 'Army' },
  { letter: 'L', name: 'Army group / front' },
  { letter: 'M', name: 'Region / theater' },
  { letter: 'N', name: 'Command' },
]

/** Symbol bases: naval task organisations (sea surface), or the members' own function. */
export const NAVAL_BASES: { id: string; name: string; fn: string; className: string }[] = [
  { id: 'task_force', name: 'Navy task force', fn: 'GT----', className: 'TASK FORCE' },
  { id: 'task_group', name: 'Navy task group', fn: 'GG----', className: 'TASK GROUP' },
  { id: 'task_unit', name: 'Navy task unit', fn: 'GU----', className: 'TASK UNIT' },
  { id: 'task_element', name: 'Navy task element', fn: 'GE----', className: 'TASK ELEMENT' },
  { id: 'convoy', name: 'Convoy', fn: 'GC----', className: 'CONVOY' },
]
export const MEMBERS_BASE = 'members'

/** The function (positions 5–10) the members' 2525C symbols share: equal ones, else their common prefix. */
export function sharedFunction(memberSidcs: string[]): string {
  const fns = memberSidcs.filter((c) => /^[A-Z]/.test(c)).map((c) => c.slice(4, 10).padEnd(6, '-'))
  if (fns.length === 0) return '------'
  let prefix = fns[0]
  for (const f of fns.slice(1)) {
    let i = 0
    while (i < prefix.length && prefix[i] === f[i] && prefix[i] !== '-') i++
    prefix = prefix.slice(0, i)
  }
  return prefix.padEnd(6, '-')
}

/** The members' most common domain. */
export function sharedDomain(memberDomains: string[]): string | null {
  const counts = new Map<string, number>()
  for (const d of memberDomains) if (d && d !== 'unknown') counts.set(d, (counts.get(d) ?? 0) + 1)
  return [...counts.entries()].sort((a, b) => b[1] - a[1])[0]?.[0] ?? null
}

export function groupSidc(parts: {
  affiliation: string | null | undefined
  domain: string | null | undefined
  base: string
  /** The members' drawable 2525C codes. */
  memberSidcs: string[]
  echelon: string | null | undefined
  taskForce: boolean
}): string {
  const aff = AFFILIATION_LETTER[parts.affiliation ?? 'unknown'] ?? 'U'
  const naval = NAVAL_BASES.find((b) => b.id === parts.base)
  const dim = naval ? 'S' : (DIMENSION_LETTER[parts.domain ?? ''] ?? 'Z')
  const fn = naval ? naval.fn : sharedFunction(parts.memberSidcs)
  return `S${aff}${dim}P${fn}${parts.taskForce ? 'E' : '-'}${parts.echelon || '-'}---`
}
