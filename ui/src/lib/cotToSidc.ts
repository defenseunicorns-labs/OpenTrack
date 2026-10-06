/**
 * Copied from OpenStare (openstare/src/utils/cotToSidc.ts; one comment reworded) so both apps draw the same
 * symbol for a CoT type; keep the two in step.
 *
 * cotToSidc — pure CoT classification string → MIL-STD-2525C SIDC converter.
 *
 * SIDC template: S{AFFIL}{BD}P{FUNC6}{TAIL6}
 *   Position 1:    'S' — Warfighting scheme (fixed)
 *   Position 2:    Affiliation letter (U/F/H/N/A/S/P/J/K)
 *   Position 3:    Battle dimension letter (A/G/S/U/P/Z)
 *   Position 4:    'P' — Present status (fixed for live tracks)
 *   Positions 5–10: Function ID (6 chars, hyphen-padded)
 *   Positions 11–16: 6 trailing hyphens (modifier/echelon unused)
 *
 * Total: 16 characters.
 *
 * Null-return contract (SYM-04 fallback):
 *   Returns null when classification is absent or too short to parse (< 5 chars).
 *   Callers must fall back to their own SYM-04 default when null is returned (a
 *   neutral themed dot for the Track live-tail path, a plain circle marker for the
 *   generic vector display/edit paths) — never trackIcons.ts's per-domain silhouette,
 *   which this function's callers never render.
 *
 * No runtime imports — this is a pure string-transform module.
 */

/** Lowercase CoT affiliation char → uppercase SIDC affiliation letter (CONTEXT.md D-06). */
const AFFIL_MAP: Record<string, string> = {
  u: 'U', // Unknown
  f: 'F', // Friend
  h: 'H', // Hostile
  n: 'N', // Neutral
  a: 'A', // Assumed Friend
  s: 'S', // Suspect
  p: 'P', // Pending
  j: 'J', // Joker
  k: 'K', // Faker
}

/** Uppercase CoT battle-dimension char → uppercase SIDC battle-dimension letter (CONTEXT.md D-06). */
const BD_MAP: Record<string, string> = {
  A: 'A', // Air
  G: 'G', // Ground
  S: 'S', // Sea Surface
  U: 'U', // Sea Subsurface
  P: 'P', // Space
  X: 'Z', // Other → Unknown (no 2525 dimension for CoT 'X')
}

/**
 * Known CoT function segment sequences → 6-char 2525C function ID.
 * Key: hyphen-joined uppercase segment strings (e.g. 'X-M', 'E').
 * Value: exactly 6 characters, hyphen-padded (e.g. 'XM----', 'E-----').
 *
 * Seed entries: the CoT function sequences seen in practice.
 */
const FUNC_TABLE: Record<string, string> = {
  '':    '------', // no function segments
  'E':   'E-----', // equipment
  'X-M': 'XM----', // subsurface/submarine
}

/**
 * Build a 6-character function ID from CoT function-segment parts.
 * Looks up the hyphen-joined key in FUNC_TABLE first; falls back to
 * joining, slicing, and padding to 6 chars.
 *
 * @param segs - array of uppercase CoT function segments (e.g. ['X', 'M'])
 * @returns exactly 6-character function ID string
 */
function buildFunctionCode(segs: string[]): string {
  const key = segs.join('-')
  if (key in FUNC_TABLE) return FUNC_TABLE[key]
  // Graceful fallback: concatenate, truncate/pad to 6 chars
  return segs.join('').slice(0, 6).padEnd(6, '-')
}

/**
 * Convert a CoT classification string to a 16-character MIL-STD-2525C SIDC.
 *
 * Returns null when the classification is absent or too short (< 5 chars),
 * triggering the SYM-04 domain icon fallback at the call site.
 *
 * @param classification - CoT classification string (e.g. 'a-u-A', 'a-u-S-X-M')
 * @returns 16-character SIDC string, or null for SYM-04 fallback
 */
export function cotToSidc(classification: string): string | null {
  if (!classification || classification.length < 5) return null

  const parts = classification.split('-')
  const affil = AFFIL_MAP[parts[1]] ?? 'U'
  const bd    = BD_MAP[parts[2]] ?? 'Z'
  const func6 = buildFunctionCode(parts.slice(3))

  return `S${affil}${bd}P${func6}------`
}
