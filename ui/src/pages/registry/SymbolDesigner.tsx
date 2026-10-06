import { useMemo, useState } from 'react'
import { TbCheck } from 'react-icons/tb'
import { Button, Label, Modal, Select } from '@kineticlogic/staresdk'
import { AFFILIATION_COLOR } from '../../lib/palette'
import { symbolUrl } from '../../lib/symbol'
import { cotForSidc } from '../../lib/milsym/sync'
import { buildSidc, parseSidc, type Affiliation } from '../../lib/milsym/sidc'
import { entities, NO_SELECTION, selectedCode, selectionFor, subtypes, symbolSets, types, type CatalogOption, type Selection } from '../../lib/milsym/catalog'

const AFFILIATIONS: { value: Affiliation; label: string }[] = [
  { value: 'unknown', label: 'Unknown' },
  { value: 'friend', label: 'Friend' },
  { value: 'neutral', label: 'Neutral' },
  { value: 'hostile', label: 'Hostile' },
]

const PREVIEW_PX = 64
const options = (list: CatalogOption[]) => list.map((o) => ({ value: o.code, label: o.label }))

interface Design {
  symbolSet: string
  selection: Selection
  affiliation: Affiliation
}

/** The design a SIDC was made from, else a blank land unit (affiliation from the entity's, if given). */
function designFor(sidc: string, affiliation?: string): Design {
  const p = parseSidc(sidc)
  if (p) return { symbolSet: p.symbolSet, selection: selectionFor(p.symbolSet, p.code), affiliation: p.affiliation }
  const a = AFFILIATIONS.find((x) => x.value === affiliation)?.value ?? 'unknown'
  return { symbolSet: '10', selection: NO_SELECTION, affiliation: a }
}

/**
 * Design a MIL-STD-2525D point symbol: affiliation, then symbol set, entity, type and subtype,
 * with a live preview. **Use symbol** hands back the 20-digit SIDC and the matching
 * Cursor-on-Target type (from the 2525C crosswalk). Started from OpenStare's tactical symbol
 * builder; point symbols only (no lines or areas), and nothing is placed on a map.
 */
export function SymbolDesigner({
  sidc,
  affiliation,
  onUse,
  onClose,
}: {
  /** The SIDC to start from (reopens its design when it is a 2525D code). */
  sidc: string
  /** The entity's affiliation, to start a new design with. */
  affiliation?: string
  onUse: (symbol: { sidc: string; cot: string }) => void
  onClose: () => void
}) {
  const [d, setD] = useState<Design>(() => designFor(sidc, affiliation))
  const sets = useMemo(() => symbolSets(), [])
  const entityList = useMemo(() => entities(d.symbolSet), [d.symbolSet])
  const typeList = useMemo(() => (d.selection.entity ? types(d.symbolSet, d.selection.entity) : []), [d.symbolSet, d.selection.entity])
  const subtypeList = useMemo(
    () => (d.selection.entity && d.selection.type ? subtypes(d.symbolSet, d.selection.entity, d.selection.type) : []),
    [d.symbolSet, d.selection.entity, d.selection.type],
  )

  const code = buildSidc(d.symbolSet, selectedCode(d.selection), d.affiliation)
  const cot = cotForSidc(code) ?? ''
  const preview = symbolUrl({ standard: '2525d', code }, PREVIEW_PX)
  const pick = (list: CatalogOption[], c: string | null) => list.find((o) => o.code === c) ?? null

  return (
    <Modal title="Symbol designer" onClose={onClose} width={460} resizable={false}>
      <div className="stack symbol-designer">
        <div className="symbol-preview">
          {preview ? <img src={preview} width={PREVIEW_PX} height={PREVIEW_PX} alt="" /> : <span className="muted">No symbol</span>}
          <span className="mono">{code}</span>
          <span className="mono muted">{cot}</span>
        </div>
        <div className="stack" style={{ gap: 4 }}>
          <Label size="sm">Affiliation</Label>
          <div className="value-row">
            {AFFILIATIONS.map((a) => (
              <Button
                key={a.value}
                size="sm"
                variant="ghost"
                active={d.affiliation === a.value}
                aria-pressed={d.affiliation === a.value}
                onClick={() => setD({ ...d, affiliation: a.value })}
                style={d.affiliation === a.value ? { boxShadow: `inset 0 -2px 0 ${AFFILIATION_COLOR[a.value]}` } : undefined}
              >
                {a.label}
              </Button>
            ))}
          </div>
        </div>
        <div className="stack" style={{ gap: 4 }}>
          <Label size="sm">Symbol set</Label>
          <Select
            ariaLabel="Symbol set"
            options={sets.map((s) => ({ value: s.code, label: `${s.code} ${s.label}` }))}
            value={d.symbolSet}
            onChange={(v) => v && setD({ ...d, symbolSet: v, selection: NO_SELECTION })}
          />
        </div>
        <div className="stack" style={{ gap: 4 }}>
          <Label size="sm">Entity</Label>
          <Select
            ariaLabel="Entity"
            placeholder="Choose an entity…"
            options={options(entityList)}
            value={d.selection.entity?.code ?? null}
            onChange={(v) => setD({ ...d, selection: { entity: pick(entityList, v), type: null, subtype: null } })}
          />
        </div>
        {typeList.length > 0 && (
          <div className="stack" style={{ gap: 4 }}>
            <Label size="sm">Type</Label>
            <Select
              ariaLabel="Entity type"
              placeholder="Any type"
              options={options(typeList)}
              value={d.selection.type?.code ?? null}
              onChange={(v) => setD({ ...d, selection: { ...d.selection, type: pick(typeList, v), subtype: null } })}
            />
          </div>
        )}
        {subtypeList.length > 0 && (
          <div className="stack" style={{ gap: 4 }}>
            <Label size="sm">Subtype</Label>
            <Select
              ariaLabel="Entity subtype"
              placeholder="Any subtype"
              options={options(subtypeList)}
              value={d.selection.subtype?.code ?? null}
              onChange={(v) => setD({ ...d, selection: { ...d.selection, subtype: pick(subtypeList, v) } })}
            />
          </div>
        )}
        <div className="toolbar">
          <span className="spacer" />
          <Button size="sm" variant="secondary" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" variant="primary" icon={<TbCheck />} disabled={!d.selection.entity} onClick={() => onUse({ sidc: code, cot })}>
            Use symbol
          </Button>
        </div>
      </div>
    </Modal>
  )
}
