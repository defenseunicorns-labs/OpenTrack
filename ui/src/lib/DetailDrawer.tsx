import type { ReactNode } from 'react'
import { TbX } from 'react-icons/tb'
import { Button, SideNav } from '@kineticlogic/staresdk'

/**
 * A detail view in a right SideNav (stareSDK DESIGN-CONTRACT §2): a header row with the
 * item's title, its actions and a close control, over a scrolling body. Composition only;
 * the drawer itself is stareSDK's SideNav.
 */
export function DetailDrawer({
  open,
  title,
  label,
  status,
  actions,
  onClose,
  storageKey,
  width = 640,
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
  storageKey: string
  width?: number
  children: ReactNode
}) {
  return (
    <SideNav open={open} ariaLabel={label} width={width} minWidth={420} maxWidth={1400} storageKey={storageKey}>
      <div
        className="drawer"
        onKeyDown={(e) => {
          if (e.key === 'Escape') onClose()
        }}
      >
        <div className="drawer-head">
          <h2 className="drawer-title">{title}</h2>
          {status}
          <span className="spacer" />
          {actions}
          <Button size="xs" variant="ghost" icon={<TbX />} aria-label="Close" title="Close" onClick={onClose} />
        </div>
        <div className="drawer-body">{children}</div>
      </div>
    </SideNav>
  )
}
