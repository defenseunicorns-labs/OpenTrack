import { useState } from 'react'
import { TbChevronRight } from 'react-icons/tb'
import type { GuideHeading } from '../../lib/help'

interface Branch {
  heading: GuideHeading
  children: GuideHeading[]
}

/** Sections (level 2), each with its sub-sections (level 3); the title is left out. */
function branches(headings: GuideHeading[]): Branch[] {
  const out: Branch[] = []
  for (const h of headings) {
    if (h.level === 2) out.push({ heading: h, children: [] })
    else if (h.level === 3 && out.length) out[out.length - 1].children.push(h)
  }
  return out
}

/**
 * A guide's contents, as the CODEX shows an article's: sections with sub-sections are branches,
 * collapsed except the one holding the current section; the heading text opens the section.
 */
export function HelpContents({
  headings,
  current,
  hrefFor,
  onOpen,
}: {
  headings: GuideHeading[]
  /** The anchor of the section open now. */
  current: string
  hrefFor: (anchor: string) => string
  onOpen: (anchor: string) => void
}) {
  const tree = branches(headings)
  // Branches the reader opened (true) or closed (false); others follow the current section.
  const [toggled, setToggled] = useState<Map<string, boolean>>(() => new Map())
  if (!tree.length) return null

  const link = (h: GuideHeading, sub: boolean) => (
    <a
      key={h.anchor}
      href={hrefFor(h.anchor)}
      className={`help-toc-link${sub ? ' help-toc-sub' : ''}`}
      aria-current={h.anchor === current ? 'location' : undefined}
      onClick={(e) => {
        e.preventDefault()
        onOpen(h.anchor)
      }}
    >
      {h.text}
    </a>
  )

  return (
    <nav className="help-toc" aria-label="Contents">
      <div className="help-rail-label">Contents</div>
      <hr className="help-rail-rule" />
      {tree.map(({ heading, children }) => {
        if (!children.length) return link(heading, false)
        const holdsCurrent = heading.anchor === current || children.some((c) => c.anchor === current)
        const open = toggled.get(heading.anchor) ?? holdsCurrent
        return (
          <div key={heading.anchor} className="help-toc-branch">
            <div className="help-toc-row">
              <button
                type="button"
                className="help-toc-toggle"
                aria-expanded={open}
                aria-label={`${open ? 'Collapse' : 'Expand'} ${heading.text}`}
                onClick={() => setToggled((m) => new Map(m).set(heading.anchor, !open))}
              >
                <TbChevronRight size={14} className={`help-toc-caret${open ? ' help-toc-caret--open' : ''}`} />
              </button>
              {link(heading, false)}
            </div>
            {open && children.map((c) => link(c, true))}
          </div>
        )
      })}
    </nav>
  )
}
