import { useState } from 'react'
import { createPortal } from 'react-dom'
import { TbInfoCircle } from 'react-icons/tb'

const WIDTH = 320

/** A small ⓘ that explains a field on hover or focus, in a padded popover beside it. */
export function InfoTip({ label, children }: { label: string; children: React.ReactNode }) {
  const [at, setAt] = useState<{ left: number; top: number } | null>(null)
  const show = (e: React.MouseEvent | React.FocusEvent) => {
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect()
    const right = r.right + 8 + WIDTH <= window.innerWidth - 8
    setAt({
      left: right ? r.right + 8 : Math.max(8, r.left - 8 - WIDTH),
      top: Math.max(8, r.top - 6),
    })
  }
  const hide = () => setAt(null)
  return (
    <span className="info-tip" tabIndex={0} role="button" aria-label={`About ${label}`} onMouseEnter={show} onMouseLeave={hide} onFocus={show} onBlur={hide}>
      <TbInfoCircle aria-hidden />
      {at &&
        createPortal(
          <div role="tooltip" className="info-pop" style={{ left: at.left, top: at.top, maxWidth: WIDTH }}>
            {children}
          </div>,
          document.body,
        )}
    </span>
  )
}
