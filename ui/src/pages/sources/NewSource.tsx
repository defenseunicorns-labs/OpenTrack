import { useState } from 'react'
import { Button, Input, Label, Modal, Select } from '@kineticlogic/staresdk'
import { api } from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { errorMessage } from '../../lib/format'
import { idFromName, newSourceSpec, TRANSPORTS, validId } from './flow/sourceFlow'

/**
 * Add a source: its name, id and transport. It is saved as a draft with its Source, Decode, Map and
 * Output steps, and opens in the editor.
 */
export function NewSource({ onClose, onCreated }: { onClose: () => void; onCreated: (id: string) => void }) {
  const [name, setName] = useState('')
  const [id, setId] = useState('')
  const [idEdited, setIdEdited] = useState(false)
  const [transport, setTransport] = useState('http_poll')
  const [creating, setCreating] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const shownId = idEdited ? id : idFromName(name)
  const ready = name.trim() !== '' && validId(shownId)

  const create = async () => {
    setCreating(true)
    setError(null)
    try {
      const row = await api.createSource(newSourceSpec(shownId, name.trim(), transport))
      onCreated(row.id)
    } catch (e) {
      setError(errorMessage(e))
      setCreating(false)
    }
  }

  return (
    <Modal title="Add source" onClose={onClose} resizable={false}>
      <form
        className="stack"
        onSubmit={(e) => {
          e.preventDefault()
          if (ready && !creating) void create()
        }}
      >
        <div className="field">
          <Label size="sm" htmlFor="new-source-name">
            Name
          </Label>
          <Input id="new-source-name" autoFocus value={name} onChange={(e) => setName(e.target.value)} />
        </div>
        <div className="field">
          <div className="row-label">
            <Label size="sm" htmlFor="new-source-id">
              Id
            </Label>
            <InfoTip label="Id">
              Names the source&apos;s data and prefixes its track ids, so it never changes. Lowercase letters, digits, - and _, at most 64.
            </InfoTip>
          </div>
          <Input
            id="new-source-id"
            className="mono"
            value={shownId}
            error={shownId !== '' && !validId(shownId)}
            onChange={(e) => {
              setIdEdited(true)
              setId(e.target.value)
            }}
            spellCheck={false}
          />
        </div>
        <div className="field">
          <Label size="sm" htmlFor="new-source-transport">
            Source
          </Label>
          <Select
            id="new-source-transport"
            ariaLabel="Source"
            options={Object.entries(TRANSPORTS).map(([value, t]) => ({ value, label: t.label }))}
            value={transport}
            onChange={(v) => v && setTransport(v)}
          />
        </div>
        {error && <div className="error-text">{error}</div>}
        <div className="row" style={{ justifyContent: 'flex-end', gap: 8 }}>
          <Button size="sm" variant="ghost" type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" type="submit" disabled={!ready || creating}>
            {creating ? 'Adding…' : 'Add'}
          </Button>
        </div>
      </form>
    </Modal>
  )
}
