/**
 * Links into the in-app Help page (the guides in docs/guides/), and how their section anchors are
 * made. Kept free of the guides themselves, so any component can link to a section cheaply:
 *
 *   <InfoTip label="Merge">… <a href={helpHref('operator', 'merge')}>More</a></InfoTip>
 *
 * Anchor rule (the one GitHub uses, so the same links work on GitHub and in the app): a heading's
 * anchor is its text in lowercase, with every character that is not a letter, digit, space, hyphen
 * or underscore removed, and each space turned into a hyphen. A repeated anchor gets `-1`, `-2`…
 * An `<a id="…"></a>` line just above a heading sets its anchor instead.
 */

/** The guides the Help page shows, in its tab order. */
export const GUIDES = [
  { id: 'operator', label: 'Operator guide', file: 'operator.md' },
  { id: 'admin', label: 'Administrator guide', file: 'admin.md' },
] as const

export type GuideId = (typeof GUIDES)[number]['id']

export const isGuide = (s: string): s is GuideId => GUIDES.some((g) => g.id === s)

/** The Help page's view id in the URL hash (`#help/<guide>/<anchor>`). */
export const HELP_VIEW = 'help'

/** A link to a guide, or to one section of it: `#help/<guide>` or `#help/<guide>/<anchor>`. */
export function helpHref(guide: GuideId, anchor?: string): string {
  return anchor ? `#${HELP_VIEW}/${guide}/${encodeURIComponent(anchor)}` : `#${HELP_VIEW}/${guide}`
}

/** The guide and anchor a Help sub-path names (`operator/merge` → operator, merge). */
export function parseHelpPath(sub: string): { guide: GuideId | null; anchor: string } {
  const [g = '', ...rest] = sub.split('/')
  return { guide: isGuide(g) ? g : null, anchor: rest.join('/') }
}

/** A heading's anchor from its text (see the rule above). */
export function slugify(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s_-]/gu, '')
    .trim()
    .replace(/\s/g, '-')
}

/** The DOM id of a guide section: prefixed, so it never clashes with the rest of the app. */
export const helpDomId = (guide: GuideId, anchor: string) => `help-${guide}-${anchor}`

export interface GuideHeading {
  /** 1 for the title, 2 for a section, 3 and 4 for sub-sections. */
  level: number
  text: string
  anchor: string
  /** 1-based line of the heading in the markdown. */
  line: number
}

const FENCE = /^\s{0,3}(```|~~~)/
const ATX = /^\s{0,3}(#{1,6})\s+(.*?)(?:\s+#+)?\s*$/
const EXPLICIT = /^\s*<a id="([^"]+)"><\/a>\s*$/

/** Heading text as it reads: inline code, emphasis and links reduced to their text. */
function plain(md: string): string {
  return md
    .replace(/\[([^\]]*)\]\([^)]*\)/g, '$1')
    .replace(/[`*]/g, '')
    .trim()
}

/** Every heading of a guide with its anchor, in order (headings in code blocks are skipped). */
export function guideHeadings(markdown: string): GuideHeading[] {
  const out: GuideHeading[] = []
  const seen = new Map<string, number>()
  let fence: string | null = null
  let explicit: string | null = null
  markdown.split(/\r?\n/).forEach((raw, i) => {
    const f = raw.match(FENCE)
    if (f) {
      if (fence === null) fence = f[1]
      else if (f[1] === fence) fence = null
      return
    }
    if (fence !== null) return
    const x = raw.match(EXPLICIT)
    if (x) {
      explicit = x[1]
      return
    }
    const h = raw.match(ATX)
    if (!h) {
      if (raw.trim()) explicit = null
      return
    }
    const text = plain(h[2])
    let anchor = explicit ?? slugify(text)
    explicit = null
    const n = seen.get(anchor)
    seen.set(anchor, (n ?? -1) + 1)
    if (n !== undefined) anchor = `${anchor}-${n + 1}`
    out.push({ level: h[1].length, text, anchor, line: i + 1 })
  })
  return out
}

/** The markdown as the Help page renders it: `<a id>` lines blanked (line numbers kept). */
export function renderableGuide(markdown: string): string {
  return markdown
    .split(/\r?\n/)
    .map((l) => (EXPLICIT.test(l) ? '' : l))
    .join('\n')
}
