import type { ReactNode } from 'react'
import { TbX } from 'react-icons/tb'
import { Button, SideNav } from '@kineticlogic/staresdk'

/**
 * A detail view with a header and scrolling body. Uses stareSDK's right SideNav for page
 * drawers, or stays inside a positioned workspace when inline.
 */
export function DetailDrawer({
  open,
  title,
  label,
  status,
  actions,
  onClose,
  storageKey,
  inline = false,
  width = inline ? 440 : 640,
  cancel = false,
  children,
}: {
  open: boolean
  title: ReactNode
  /** Accessible name of the drawer, e.g. "Source detail". */
  label: string
  /** Shown after the title (a state badge). */
  status?: ReactNode
  actions?: ReactNode
  onClose: () => void
  storageKey?: string
  width?: number
  inline?: boolean
  cancel?: boolean
  children: ReactNode
}) {
  const content = (
    <div
      className="drawer"
      inert={!open}
      onKeyDown={(e) => {
        if (e.key === 'Escape' && !e.defaultPrevented) onClose()
      }}
    >
      <div className="drawer-head">
        <h2 className="drawer-title">{title}</h2>
        {status}
        <span className="spacer" />
        {actions}
        {cancel ? (
          <Button size="sm" variant="ghost" onClick={onClose}>Cancel</Button>
        ) : (
          <Button size="xs" variant="ghost" icon={<TbX />} aria-label="Close" title="Close" onClick={onClose} />
        )}
      </div>
      <div className="drawer-body">{children}</div>
    </div>
  )

  if (inline) {
    return (
      <aside className="drawer-inline" aria-label={label} aria-hidden={!open} style={{ width }}>
        {content}
      </aside>
    )
  }

  return (
    <SideNav open={open} ariaLabel={label} width={width} minWidth={420} maxWidth={1400} storageKey={storageKey}>
      {content}
    </SideNav>
  )
}
