import { GUIDES, type GuideId } from '../../lib/help'

/**
 * The guides' markdown, bundled at build time from docs/guides/, so Help works offline. A build
 * without that directory (an image that copies only ui/) still builds, with no guides.
 */
const files = import.meta.glob<string>('../../../../docs/guides/*.md', { query: '?raw', import: 'default', eager: true })

const byName = new Map(Object.entries(files).map(([path, text]) => [path.slice(path.lastIndexOf('/') + 1), text]))

/** A guide's markdown, or null when this build has none. */
export function guideMarkdown(id: GuideId): string | null {
  const g = GUIDES.find((x) => x.id === id)
  return (g && byName.get(g.file)) ?? null
}
