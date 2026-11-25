import { useRef, useState } from 'react'
import { TbChevronDown } from 'react-icons/tb'
import { Badge, Button, Popover } from '@kineticlogic/staresdk'
import type { SchemaVersion } from '../../api/client'

type Version = Pick<SchemaVersion, 'version' | 'status'>

function VersionLabel({ version }: { version: Version }) {
  return (
    <span className="schema-version-label">
      <span>Version {version.version}</span>
      {version.status === 'draft' && <Badge size="sm" color="warning" uppercase>Draft</Badge>}
    </span>
  )
}

export function SchemaVersionSelect({ versions, selected, disabled, onSelect }: {
  versions: Version[]
  selected: number | null
  disabled: boolean
  onSelect: (version: number) => void
}) {
  const [open, setOpen] = useState(false)
  const anchor = useRef<HTMLButtonElement>(null)
  const current = versions.find((version) => version.version === selected)
  return (
    <>
      <Button
        size="sm"
        variant="secondary"
        disabled={disabled || !current}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={(event) => {
          anchor.current = event.currentTarget
          setOpen(!open)
        }}
      >
        <span className="schema-version-label">
          {current ? <VersionLabel version={current} /> : 'Version'}
          <TbChevronDown aria-hidden />
        </span>
      </Button>
      <Popover open={open} onClose={() => setOpen(false)} anchorRef={anchor} ariaLabel="Schema versions" minWidth={155}>
        {versions.map((version) => (
          <Button
            key={version.version}
            size="sm"
            variant="ghost"
            fullWidth
            justify="flex-start"
            aria-current={version.version === selected || undefined}
            onClick={() => { setOpen(false); onSelect(version.version) }}
          >
            <VersionLabel version={version} />
          </Button>
        ))}
      </Popover>
    </>
  )
}
