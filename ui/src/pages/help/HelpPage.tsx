import { useEffect, useMemo, type ReactNode } from 'react'
import { Facets, MDText, type Components } from '@kineticlogic/staresdk'
import { GUIDES, guideHeadings, helpDomId, helpHref, isGuide, parseHelpPath, renderableGuide, type GuideId } from '../../lib/help'
import { guideMarkdown } from './guides'
import { HelpContents } from './HelpContents'
import './help.css'

/** Where a link in a guide goes: a section here, the other guide, the repository, or the web. */
function resolveLink(guide: GuideId, href: string): { kind: 'help'; href: string } | { kind: 'repo'; path: string } | { kind: 'web'; href: string } {
  if (href.startsWith('#')) return { kind: 'help', href: helpHref(guide, href.slice(1)) }
  if (/^[a-z][a-z0-9+.-]*:/i.test(href)) return { kind: 'web', href }
  const [path, anchor] = href.split('#')
  const file = path.replace(/^\.\//, '')
  const other = GUIDES.find((g) => g.file === file)
  if (other) return { kind: 'help', href: helpHref(other.id, anchor) }
  // Relative to docs/guides/: a file in the repository, which the app does not serve.
  const parts = ['docs', 'guides']
  for (const p of path.split('/')) {
    if (p === '..') parts.pop()
    else if (p && p !== '.') parts.push(p)
  }
  return { kind: 'repo', path: parts.join('/') }
}

/**
 * Help: the operator and administrator guides (docs/guides/*.md, bundled at build time), with a
 * section list. `#help/<guide>/<anchor>` opens a guide at a section; see `helpHref`.
 */
export default function HelpPage({ sub }: { sub: string }) {
  const parsed = parseHelpPath(sub)
  const guide: GuideId = parsed.guide ?? 'operator'
  const anchor = parsed.anchor
  const markdown = guideMarkdown(guide)
  const headings = useMemo(() => (markdown ? guideHeadings(markdown) : []), [markdown])
  const byLine = useMemo(() => new Map(headings.map((h) => [h.line, h])), [headings])
  // Each guide with its number of sections, for the Guide facet.
  const guideOptions = useMemo(
    () =>
      GUIDES.map((g) => {
        const md = guideMarkdown(g.id)
        return { value: g.id, label: g.label, count: md ? guideHeadings(md).filter((h) => h.level === 2).length : 0 }
      }),
    [],
  )

  // Scroll to the section the address names (or to the top of a guide), once it is on the page.
  useEffect(() => {
    const id = anchor ? helpDomId(guide, anchor) : `help-${guide}-top`
    const frame = requestAnimationFrame(() => document.getElementById(id)?.scrollIntoView({ block: 'start' }))
    return () => cancelAnimationFrame(frame)
  }, [guide, anchor, markdown])

  const components: Components = useMemo(() => {
    const heading =
      (Tag: 'h1' | 'h2' | 'h3' | 'h4') =>
      ({ node, children }: { node?: { position?: { start: { line: number } } }; children?: ReactNode }) => {
        const h = node?.position ? byLine.get(node.position.start.line) : undefined
        return (
          <Tag id={h ? helpDomId(guide, h.anchor) : undefined} className="help-heading">
            {children}
            {h && Tag !== 'h1' && (
              <a className="help-anchor" href={helpHref(guide, h.anchor)} aria-label={`Link to ${h.text}`} title="Link to this section">
                #
              </a>
            )}
          </Tag>
        )
      }
    return {
      h1: heading('h1'),
      h2: heading('h2'),
      h3: heading('h3'),
      h4: heading('h4'),
      a: ({ href = '', children }) => {
        const to = resolveLink(guide, href)
        if (to.kind === 'help') return <a href={to.href}>{children}</a>
        if (to.kind === 'web')
          return (
            <a href={to.href} target="_blank" rel="noreferrer">
              {children}
            </a>
          )
        return (
          <span className="help-repo-link" title={`In the OpenTrack repository: ${to.path}`}>
            {children}
          </span>
        )
      },
      table: ({ children }) => (
        <div className="help-table">
          <table>{children}</table>
        </div>
      ),
    }
  }, [guide, byLine])

  const go = (g: GuideId, a?: string) => {
    const to = helpHref(g, a)
    // The same address again: no hashchange, so scroll here.
    if (window.location.hash === to) document.getElementById(a ? helpDomId(g, a) : `help-${g}-top`)?.scrollIntoView({ block: 'start' })
    else window.location.hash = to
  }
  const file = GUIDES.find((g) => g.id === guide)!.file

  return (
    <div className="help-layout">
      <aside className="help-rail">
        <Facets
          label="Guide"
          options={guideOptions}
          selected={[guide]}
          onToggle={(id) => isGuide(id) && go(id)}
          // One guide is always open: Clear goes back to the first.
          onClear={() => go(GUIDES[0].id)}
        />
        {/* Keyed by guide, so branches opened in one guide don't carry over to the other. */}
        <HelpContents key={guide} headings={headings} current={anchor} hrefFor={(a) => helpHref(guide, a)} onOpen={(a) => go(guide, a)} />
      </aside>
      <article className="help-doc" id={`help-${guide}-top`}>
        {markdown === null ? (
          <p className="muted">This build of OpenTrack has no guides. They are in the OpenTrack repository, in docs/guides/{file}.</p>
        ) : (
          <MDText markdown={renderableGuide(markdown)} components={components} />
        )}
      </article>
    </div>
  )
}
