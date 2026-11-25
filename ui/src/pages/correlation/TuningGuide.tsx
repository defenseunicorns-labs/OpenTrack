import { useRef, useState } from 'react'
import { Button, Popover, PopoverMenuButton } from '@kineticlogic/staresdk'
import { TbAdjustmentsHorizontal } from 'react-icons/tb'
import type { CorrelationSettings } from '../../api/client'
import type { TuningTarget } from './TuningCase'

export function TuningGuide({ draft, onSelect }: {
  draft: CorrelationSettings
  onSelect: (target: TuningTarget) => void
}) {
  const [open, setOpen] = useState(false)
  const anchor = useRef<HTMLButtonElement>(null)
  const kinematic = draft.approach !== 'identifiers'
  const splits = draft.split.propose || draft.split.automatic
  const select = (target: TuningTarget) => {
    setOpen(false)
    onSelect(target)
  }

  return (
    <>
      <Button
        size="sm"
        variant="ghost"
        icon={<TbAdjustmentsHorizontal />}
        aria-haspopup="dialog"
        aria-expanded={open}
        onClick={(event) => {
          anchor.current = event.currentTarget
          setOpen(!open)
        }}
      >
        Tuning guide
      </Button>
      <Popover open={open} onClose={() => setOpen(false)} anchorRef={anchor} ariaLabel="Tuning guide" align="end" minWidth={250} estimatedHeight={116}>
        <PopoverMenuButton onClick={() => select(kinematic ? 'pair' : 'identifier')}>
          Different objects are being paired
        </PopoverMenuButton>
        <PopoverMenuButton onClick={() => select(kinematic ? 'comparisons' : 'identifier')}>
          The same object stays on two tracks
        </PopoverMenuButton>
        {splits && <PopoverMenuButton onClick={() => select('split')}>Tracks split too often</PopoverMenuButton>}
      </Popover>
    </>
  )
}
