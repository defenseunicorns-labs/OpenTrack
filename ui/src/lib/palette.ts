/**
 * Data colours for track affiliation (theme-invariant, like OpenStare's map feature colours).
 * Validated for contrast and colour-vision separation on both basemap themes in trackview.
 */
export const AFFILIATION_COLOR: Record<string, string> = {
  friend: '#3987e5',
  assumed_friend: '#3987e5',
  hostile: '#d03b3b',
  suspect: '#d03b3b',
  joker: '#d03b3b',
  faker: '#d03b3b',
  neutral: '#3d9a6a',
  unknown: '#c98500',
  pending: '#c98500',
  none: '#8a8f98',
}

export function affiliationColor(affiliation: string | undefined, cotType?: string): string {
  if (affiliation && AFFILIATION_COLOR[affiliation]) return AFFILIATION_COLOR[affiliation]
  const atom = cotType?.split('-')[1]
  const byAtom: Record<string, string> = { f: 'friend', a: 'friend', h: 'hostile', s: 'hostile', n: 'neutral', u: 'unknown', p: 'pending' }
  return AFFILIATION_COLOR[byAtom[atom ?? ''] ?? 'none']
}

/** Non-point evidence on the track map (theme-invariant data colours): bearing lines and sensor
 *  positions, and the paler uncertainty (bearing wedges, position ellipses). */
export const EVIDENCE_COLOR = '#ffd166'
export const UNCERTAINTY_COLOR = '#ffe49a'
