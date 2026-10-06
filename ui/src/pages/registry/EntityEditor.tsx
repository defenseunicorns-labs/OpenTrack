import { useEffect, useMemo, useState } from 'react'
import { TbFlag3, TbPlus, TbTrash, TbX } from 'react-icons/tb'
import { Badge, Button, DataTable, FieldSelect, Input, SaveButton, useToast, type DataTableColumn } from '@kineticlogic/staresdk'
import {
  AFFILIATIONS,
  ATTR_TYPES,
  DOMAINS,
  TRACK_TYPES,
  api,
  type AttrType,
  type Entity,
  type EntityView,
} from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { DetailDrawer } from '../../lib/DetailDrawer'
import { ago, errorMessage, fmtTime, show } from '../../lib/format'
import { INPUT } from '../../lib/valueSpec'
import { useCan } from '../../auth/context'
import { SymbolDesigner } from './SymbolDesigner'
import { syncSymbol } from '../../lib/milsym/sync'

type Revision = EntityView['revisions'][number]
type LinkedTrack = EntityView['tracks'][number]

/** Identifier schemes offered as suggestions; any scheme may be typed. */
const SCHEMES = ['mmsi', 'imo', 'icao', 'callsign', 'hull', 'elnot', 'cot-uid', 'track']

/** An attribute row being edited: its value as text. */
interface AttrDraft {
  key: string
  type: AttrType
  text: string
}

interface Draft {
  status: Entity['status']
  publish: 'automatic' | 'always' | 'never'
  minimum: Record<(typeof MINIMUM)[number]['key'], string>
  identifiers: { scheme: string; value: string; expected_name: string }[]
  attributes: AttrDraft[]
}

/** The OTH-GOLD minimum an entity carries, in form order. */
/** The identifier scheme that pins an entity to one system track (crates/ot-server/src/engine.rs). */
const TRACK_SCHEME = 'track'

const MINIMUM = [
  { key: 'name', label: 'Name', info: 'The platform name. Linked tracks publish it (in capitals) in place of the name their feed reports; blank leaves the feed\'s name.' },
  { key: 'class_name', label: 'Class name', info: 'OTH-GOLD class name, e.g. the ship or aircraft class. Published as the track class.' },
  {
    key: 'domain',
    label: 'Domain',
    options: DOMAINS,
    info: 'Where the platform operates: air, surface, subsurface, ground or space. Blank leaves the feed\'s value; with none, it is derived from the CoT type or SIDC, else published as unknown. Changing it to another domain than the symbol\'s sets the symbol to that domain\'s generic one.',
  },
  {
    key: 'affiliation',
    label: 'Affiliation',
    options: AFFILIATIONS,
    info: 'The standard identity (friend, hostile, neutral, suspect, joker, faker…). Blank leaves the feed\'s value; with none, it is derived from the CoT type or SIDC. With the domain it sets the OTH-GOLD force code. Changing it rewrites the symbol\'s affiliation (CoT type and SIDC).',
  },
  {
    key: 'track_type',
    label: 'Track type',
    options: TRACK_TYPES,
    info: 'Tactical: a real-world track (the default). Live training: a real unit designated for training. Simulated training: made up for an exercise. Demand entry: a real unit receivers should not filter out.',
  },
  { key: 'cot_type', label: 'CoT type', info: 'Cursor-on-Target type, e.g. a-f-S-C-L. Kept in step with the SIDC: typing a CoT type sets the SIDC (its 2525D equivalent), domain and affiliation.' },
  {
    key: 'sidc',
    label: 'SIDC',
    info: 'MIL-STD-2525D symbol code, kept in step with the CoT type: typing a SIDC (a 2525C code is stored as its 2525D equivalent) sets the CoT type, domain and affiliation. The flag button opens the symbol designer.',
  },
] as const

const TRACK_COLUMNS: DataTableColumn<LinkedTrack>[] = [
  { key: 'track', header: 'Track', mono: true, width: 150, render: (t) => t.track_id },
  { key: 'source', header: 'Source', mono: true, render: (t) => t.source_id },
  { key: 'state', header: 'State', width: 90, render: (t) => t.state },
  { key: 'last', header: 'Last', width: 60, align: 'right', render: (t) => ago(t.last_seen) },
  {
    key: 'notices',
    header: 'Replaced',
    width: 80,
    align: 'right',
    render: (t) =>
      t.notices.length > 0 ? (
        <Badge
          color="warning"
          size="sm"
          title={t.notices.map((n) => `${n.key}: feed ${show(n.feed)}, entity ${show(n.entity)}`).join('\n')}
        >
          {t.notices.length}
        </Badge>
      ) : (
        ''
      ),
  },
]

function text(v: unknown): string {
  if (v === undefined || v === null) return ''
  return typeof v === 'string' ? v : JSON.stringify(v)
}

function toDraft(e: Partial<Entity>): Draft {
  return {
    status: e.status ?? 'active',
    publish: e.publish ?? 'automatic',
    minimum: Object.fromEntries(MINIMUM.map((m) => [m.key, text(e[m.key])])) as Draft['minimum'],
    identifiers: (e.identifiers ?? []).map((i) => ({ scheme: i.scheme, value: i.value, expected_name: i.expected_name ?? '' })),
    attributes: (e.attributes ?? []).map((a) => ({ key: a.key, type: a.type, text: text(a.value) })),
  }
}

/** The draft as an entity to save; JSON attributes are parsed here, other types by the server. */
function fromDraft(id: string, d: Draft): Entity {
  const blank = (s: string) => (s.trim() === '' ? null : s.trim())
  return {
    id,
    status: d.status,
    publish: d.publish === 'automatic' ? null : d.publish,
    ...(Object.fromEntries(MINIMUM.map((m) => [m.key, blank(d.minimum[m.key])])) as Partial<Entity>),
    identifiers: d.identifiers
      .filter((i) => i.scheme.trim() || i.value.trim())
      .map((i) => ({ scheme: i.scheme.trim(), value: i.value.trim(), expected_name: blank(i.expected_name) })),
    attributes: d.attributes
      .filter((a) => a.key.trim() || a.text.trim())
      .map((a) => {
        let value: unknown = blank(a.text)
        if (a.type === 'json' && value !== null) {
          try {
            value = JSON.parse(a.text)
          } catch {
            throw new Error(`attribute ${a.key}: not valid JSON`)
          }
        }
        return { key: a.key.trim(), type: a.type, value }
      }),
  } as Entity
}

/** What a revision changed from the one before it. */
function changes(r: Revision, older?: Revision): string {
  if (!older) return 'created'
  const a = r.entity as unknown as Record<string, unknown>
  const b = older.entity as unknown as Record<string, unknown>
  const out: string[] = []
  for (const k of ['status', 'publish', ...MINIMUM.map((m) => m.key)]) if (text(a[k]) !== text(b[k])) out.push(k)
  const ids = (e: Entity) => e.identifiers.map((i) => `${i.scheme}:${i.value}`)
  const [now, was] = [ids(r.entity), ids(older.entity)]
  for (const i of now) if (!was.includes(i)) out.push(`+${i}`)
  for (const i of was) if (!now.includes(i)) out.push(`−${i}`)
  const attrs = (e: Entity) => new Map(e.attributes.map((x) => [x.key, JSON.stringify([x.type, x.value])]))
  const [an, ab] = [attrs(r.entity), attrs(older.entity)]
  for (const k of new Set([...an.keys(), ...ab.keys()])) if (an.get(k) !== ab.get(k)) out.push(k)
  return out.join(', ') || 'no change'
}

function ValueInput({ a, onChange }: { a: AttrDraft; onChange: (t: string) => void }) {
  if (a.type === 'boolean') {
    return (
      <FieldSelect
        ariaLabel={`${a.key || 'attribute'} value`}
        allowNone
        fields={[{ name: 'true' }, { name: 'false' }]}
        value={a.text || null}
        onChange={(v) => onChange(v ?? '')}
        style={{ width: '100%' }}
      />
    )
  }
  return (
    <Input
      style={{ ...INPUT, width: '100%', fontFamily: a.type === 'json' ? 'var(--font-mono)' : undefined }}
      aria-label={`${a.key || 'attribute'} value`}
      type={a.type === 'number' ? 'number' : 'text'}
      step="any"
      value={a.text}
      placeholder={a.type === 'datetime' ? '2026-09-26T12:00:00Z' : a.type === 'json' ? '{"any": "JSON"}' : undefined}
      onChange={(e) => onChange(e.target.value)}
      autoComplete="off"
      spellCheck={false}
    />
  )
}

/**
 * An entity in a right drawer: status, the OTH-GOLD minimum, identifiers and free-form attributes.
 * With `entityId` it edits that entity; without one it creates a new one from `seed` (a track's
 * name and identifiers, say), and nothing is written until the user saves.
 */
export function EntityEditor({
  entityId,
  seed,
  open,
  onClose,
  onSaved,
  pinTrack,
}: {
  entityId: string | null
  seed?: Partial<Entity>
  /** Opened from this track's card (its track number): saving pins the entity to it, so it always
   *  applies to that track, through merges, whatever its feeds' reports grade as. */
  pinTrack?: string
  open: boolean
  onClose: () => void
  onSaved: (entityId: string | null) => void
}) {
  const canManage = useCan('track_manager')
  const { toast, confirm } = useToast()
  const [view, setView] = useState<EntityView | null>(null)
  const [draft, setDraft] = useState<Draft | null>(null)
  const [designing, setDesigning] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)

  useEffect(() => {
    setError(null)
    setSaved(false)
    if (!open) return
    if (entityId) {
      setView(null)
      setDraft(null)
      api.entity(entityId).then(
        (v) => {
          setView(v)
          setDraft(toDraft(v.entity))
        },
        (e) => setError(errorMessage(e)),
      )
    } else {
      setView(null)
      const d = toDraft(seed ?? {})
      if (d.identifiers.length === 0) d.identifiers.push({ scheme: '', value: '', expected_name: '' })
      setDraft(d)
    }
    // The seed is read when the drawer opens: a live track refreshing under an open form must not reset it.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [entityId, open])

  const baseline = useMemo(() => (view ? JSON.stringify(toDraft(view.entity)) : null), [view])
  const pinned = (d: Draft) => d.identifiers.some((i) => i.scheme === TRACK_SCHEME && i.value === pinTrack)
  // Not yet pinned to the track it was opened from: saving pins it, so Save is offered.
  const unpinned = !!pinTrack && draft !== null && !pinned(draft)
  const dirty = draft !== null && (baseline === null || JSON.stringify(draft) !== baseline || unpinned)

  if (!draft) {
    return (
      <DetailDrawer open={open} onClose={onClose} label="Entity editor" storageKey="ot.entityDetail.width" title={entityId ?? 'Entity'}>
        <div className="panel-body">{error ? <div className="error-text">{error}</div> : <span className="muted">LOADING…</span>}</div>
      </DetailDrawer>
    )
  }
  const set = (patch: Partial<Draft>) => {
    setSaved(false)
    setDraft({ ...draft, ...patch })
  }
  // Domain, affiliation, CoT type and SIDC follow each other (lib/milsym/sync.ts).
  const setMin = (key: keyof Draft['minimum'], v: string) => {
    const minimum = { ...draft.minimum, [key]: v }
    if (key === 'domain' || key === 'affiliation' || key === 'cot_type' || key === 'sidc') Object.assign(minimum, syncSymbol(minimum, key))
    set({ minimum })
  }
  const setIdent = (i: number, patch: Partial<Draft['identifiers'][number]>) =>
    set({ identifiers: draft.identifiers.map((x, j) => (j === i ? { ...x, ...patch } : x)) })
  const setAttr = (i: number, patch: Partial<AttrDraft>) => set({ attributes: draft.attributes.map((x, j) => (j === i ? { ...x, ...patch } : x)) })

  const save = async () => {
    setSaving(true)
    setError(null)
    try {
      const entity = fromDraft(entityId ?? '', unpinned ? { ...draft, identifiers: [...draft.identifiers, { scheme: TRACK_SCHEME, value: pinTrack, expected_name: '' }] } : draft)
      const v = entityId ? await api.saveEntity(entity) : await api.createEntity(entity)
      setView(v)
      setDraft(toDraft(v.entity))
      setSaved(true)
      onSaved(v.entity.id)
    } catch (e) {
      setError(errorMessage(e))
    } finally {
      setSaving(false)
    }
  }

  const remove = async () => {
    if (!entityId) return
    const ok = await confirm(
      `Delete ${view?.entity.name ?? entityId} and its ${draft.identifiers.length} identifier${draft.identifiers.length === 1 ? '' : 's'}? Tracks stop resolving to it. Its history stays in the decision log.`,
      { title: 'Delete entity', confirmLabel: 'Delete' },
    )
    if (!ok) return
    try {
      await api.deleteEntity(entityId)
      toast({ variant: 'success', title: 'Deleted', message: view?.entity.name ?? entityId })
      onSaved(null)
      onClose()
    } catch (e) {
      setError(errorMessage(e))
    }
  }

  const revisions = view?.revisions ?? []
  const REVISION_COLUMNS: DataTableColumn<Revision>[] = [
    { key: 'at', header: 'Saved', mono: true, width: 150, render: (r) => fmtTime(r.saved_at_ms) },
    { key: 'by', header: 'By', mono: true, width: 130, render: (r) => r.actor ?? '—' },
    { key: 'what', header: 'Changed', render: (r) => changes(r, revisions[revisions.indexOf(r) + 1]) },
  ]

  return (
    <DetailDrawer
      open={open}
      onClose={onClose}
      label="Entity editor"
      storageKey="ot.entityDetail.width"
      title={draft.minimum.name || view?.entity.id || 'New entity'}
      status={
        <Badge color={draft.status === 'active' ? (entityId ? 'grey' : 'blue') : 'warning'} size="sm" uppercase>
          {entityId ? draft.status : 'new'}
        </Badge>
      }
      actions={
        <>
          <span className="publish-pick">
            <InfoTip label="Publish">
              Automatic: the usual rules (confirmed, and reported by a source that may stand alone). Always: published at once, confirmed or
              not, whatever reports for them and whatever the output filter says. Never: kept inside OpenTrack, and withdrawn downstream if
              already published.
            </InfoTip>
            <FieldSelect
              ariaLabel="Publish"
              fields={[{ name: 'automatic' }, { name: 'always' }, { name: 'never' }]}
              value={draft.publish}
              onChange={(v) => set({ publish: v === 'always' || v === 'never' ? v : 'automatic' })}
              style={{ width: 118 }}
            />
          </span>
          {entityId && canManage && <Button size="sm" variant="ghost" icon={<TbTrash />} aria-label="Delete entity" title="Delete entity" onClick={remove} />}
          {canManage && <SaveButton size="sm" dirty={dirty} saving={saving} saved={saved} onSave={save} />}
        </>
      }
    >
      <div className="panel-body entity-editor">
        {view && <span className="muted mono">{view.entity.id}</span>}
        {error && <div className="error-text">{error}</div>}

        <h4 className="subhead">
          Identity
          <InfoTip label="Identity">
            The OTH-GOLD minimum every track publishes. The entity is the authority: sources whose pipeline links these fields publish the
            entity&apos;s values in place of what their feed reports.
          </InfoTip>
        </h4>
        <div className="entity-form">
          {MINIMUM.map((m) => (
            <div key={m.key} className="entity-form-row">
              <label className="entity-form-label" htmlFor={`entity-${m.key}`}>
                {m.label}
                {'info' in m && <InfoTip label={m.label}>{m.info}</InfoTip>}
              </label>
              {'options' in m ? (
                <FieldSelect
                  ariaLabel={m.label}
                  allowNone
                  fields={m.options.map((name) => ({ name }))}
                  value={draft.minimum[m.key] || null}
                  onChange={(v) => setMin(m.key, v ?? '')}
                  style={{ width: '100%' }}
                />
              ) : (
                <div className="value-row" style={{ flexWrap: 'nowrap' }}>
                  <Input
                    id={`entity-${m.key}`}
                    style={{ ...INPUT, width: '100%', fontFamily: m.key === 'name' ? undefined : 'var(--font-mono)' }}
                    value={draft.minimum[m.key]}
                    onChange={(e) => setMin(m.key, e.target.value)}
                    autoComplete="off"
                    spellCheck={false}
                  />
                  {m.key === 'sidc' && (
                    <Button size="xs" variant="secondary" icon={<TbFlag3 />} aria-label="Design the symbol" title="Design the symbol" onClick={() => setDesigning(true)} />
                  )}
                </div>
              )}
            </div>
          ))}
          <div className="entity-form-row">
            <label className="entity-form-label">
              Status
              <InfoTip label="Status">
                Active: tracks resolve to it by its identifiers. Retired: kept on record with its history, but no track resolves to it any
                more; its identifiers stay reserved to it, so delete them or the entity to reuse them.
              </InfoTip>
            </label>
            <FieldSelect
              ariaLabel="Status"
              fields={[{ name: 'active' }, { name: 'retired' }]}
              value={draft.status}
              onChange={(v) => set({ status: v === 'retired' ? 'retired' : 'active' })}
              style={{ width: '100%' }}
            />
          </div>
        </div>

        <div className="entity-section-head">
          <h4 className="subhead">
            Identifiers
            <InfoTip label="Identifiers">
              How tracks find this entity: any scheme (mmsi, icao, elnot, …) and value. An identifier belongs to one entity. Name is the name
              the track is expected to report under it, which corroborates the match.
            </InfoTip>
          </h4>
          <Button size="sm" variant="ghost" icon={<TbPlus />} onClick={() => set({ identifiers: [...draft.identifiers, { scheme: '', value: '', expected_name: '' }] })}>
            Add identifier
          </Button>
        </div>
        <datalist id="entity-schemes">
          {SCHEMES.map((s) => (
            <option key={s} value={s} />
          ))}
        </datalist>
        <div className="kv-table kv-identifiers">
          <span className="kv-head">Scheme</span>
          <span className="kv-head">Value</span>
          <span className="kv-head">Name</span>
          <span />
          {draft.identifiers.map((i, n) => (
            <IdentifierRow key={n} i={i} n={n} onChange={setIdent} onRemove={() => set({ identifiers: draft.identifiers.filter((_, j) => j !== n) })} />
          ))}
          {draft.identifiers.length === 0 && !unpinned && <span className="muted kv-empty">No identifiers: tracks cannot find this entity.</span>}
          {unpinned && (
            <span className="muted">
              Saving pins this entity to track <span className="mono">{pinTrack}</span> (identifier <span className="mono">track</span>): it then
              applies to that track whatever reports for it, through merges.
            </span>
          )}
        </div>

        <div className="entity-section-head">
          <h4 className="subhead">
            Attributes
            <InfoTip label="Attributes">
              Anything else about the entity, as key, type and value. A source&apos;s pipeline can publish an attribute on its tracks, or update
              it from what the feed reports (an AIS destination, say).
            </InfoTip>
          </h4>
          <Button size="sm" variant="ghost" icon={<TbPlus />} onClick={() => set({ attributes: [...draft.attributes, { key: '', type: 'text', text: '' }] })}>
            Add attribute
          </Button>
        </div>
        <div className="kv-table kv-attributes">
          <span className="kv-head">Key</span>
          <span className="kv-head">
            Type
            <InfoTip label="Attribute type">
              How the value is read and published: text, number, boolean (true/false), datetime (RFC 3339, e.g. 2026-09-26T12:00:00Z) or
              json (any JSON value). A value that does not parse as its type is refused on save.
            </InfoTip>
          </span>
          <span className="kv-head">Value</span>
          <span />
          {draft.attributes.map((a, n) => (
            <div key={n} className="kv-row">
              <Input
                style={{ ...INPUT, width: '100%', fontFamily: 'var(--font-mono)' }}
                aria-label={`Attribute ${n + 1} key`}
                value={a.key}
                placeholder="key"
                onChange={(e) => setAttr(n, { key: e.target.value })}
                autoComplete="off"
                spellCheck={false}
              />
              <FieldSelect
                ariaLabel={`Attribute ${n + 1} type`}
                fields={ATTR_TYPES.map((name) => ({ name }))}
                value={a.type}
                onChange={(t) => setAttr(n, { type: (t ?? 'text') as AttrType })}
                style={{ width: '100%' }}
              />
              <ValueInput a={a} onChange={(t) => setAttr(n, { text: t })} />
              <Button
                size="xs"
                variant="ghost"
                icon={<TbX />}
                aria-label={`Remove attribute ${a.key || n + 1}`}
                onClick={() => set({ attributes: draft.attributes.filter((_, j) => j !== n) })}
              />
            </div>
          ))}
          {draft.attributes.length === 0 && <span className="muted kv-empty">No attributes.</span>}
        </div>

        {view && (
          <>
            <h4 className="subhead">
              Live tracks
              <InfoTip label="Live tracks">
                Tracks resolving to this entity now. &ldquo;Replaced&rdquo; counts fields where the entity&apos;s value was published in place of
                a different one the feed reported; hover a count to see the fields and both values.
              </InfoTip>
            </h4>
            <DataTable
              aria-label="Live tracks resolving to this entity"
              columns={TRACK_COLUMNS}
              rows={view.tracks}
              rowKey={(t) => t.uid}
              maxHeight={200}
              empty="No live track resolves to this entity right now."
            />
            <h4 className="subhead">
              History
              <InfoTip label="History">
                Every saved revision of this entity, newest first: when, by whom, and which fields, identifiers (+ added, − removed) and
                attributes changed.
              </InfoTip>
            </h4>
            <DataTable aria-label="Entity history" columns={REVISION_COLUMNS} rows={revisions} rowKey={(r) => String(r.id)} maxHeight={200} empty="No changes recorded since the registry moved to entities." />
          </>
        )}
      </div>
      {designing && (
        <SymbolDesigner
          sidc={draft.minimum.sidc}
          affiliation={draft.minimum.affiliation}
          onClose={() => setDesigning(false)}
          onUse={({ sidc }) => {
            setMin('sidc', sidc)
            setDesigning(false)
          }}
        />
      )}
    </DetailDrawer>
  )
}

function IdentifierRow({
  i,
  n,
  onChange,
  onRemove,
}: {
  i: Draft['identifiers'][number]
  n: number
  onChange: (n: number, patch: Partial<Draft['identifiers'][number]>) => void
  onRemove: () => void
}) {
  return (
    <div className="kv-row">
      <Input
        style={{ ...INPUT, width: '100%', fontFamily: 'var(--font-mono)' }}
        aria-label={`Identifier ${n + 1} scheme`}
        list="entity-schemes"
        value={i.scheme}
        placeholder="mmsi"
        onChange={(e) => onChange(n, { scheme: e.target.value })}
        autoComplete="off"
        spellCheck={false}
      />
      <Input
        style={{ ...INPUT, width: '100%', fontFamily: 'var(--font-mono)' }}
        aria-label={`Identifier ${n + 1} value`}
        value={i.value}
        onChange={(e) => onChange(n, { value: e.target.value })}
        autoComplete="off"
        spellCheck={false}
      />
      <Input
        style={{ ...INPUT, width: '100%' }}
        aria-label={`Identifier ${n + 1} broadcast name`}
        value={i.expected_name}
        placeholder="optional"
        onChange={(e) => onChange(n, { expected_name: e.target.value })}
        autoComplete="off"
        spellCheck={false}
      />
      <Button size="xs" variant="ghost" icon={<TbX />} aria-label={`Remove identifier ${i.scheme}:${i.value}`} onClick={onRemove} />
    </div>
  )
}
