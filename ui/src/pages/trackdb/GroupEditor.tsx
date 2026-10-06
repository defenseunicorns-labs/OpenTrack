import { useEffect, useMemo, useState } from 'react'
import { TbTrash, TbX } from 'react-icons/tb'
import { Badge, Button, FieldSelect, Input, SaveButton, Toggle, useToast } from '@kineticlogic/staresdk'
import { AFFILIATIONS, DOMAINS, api, type GroupSpec, type TrackGroup, type TrackRow } from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { DetailDrawer } from '../../lib/DetailDrawer'
import { errorMessage } from '../../lib/format'
import { ECHELONS, MEMBERS_BASE, NAVAL_BASES, groupSidc, sharedDomain } from '../../lib/groupSymbol'
import { drawableSidc, symbolUrl } from '../../lib/symbol'
import { INPUT } from '../../lib/valueSpec'
import { useCan } from '../../auth/context'

const BASES = [{ id: MEMBERS_BASE, name: "Members' own symbol" }, ...NAVAL_BASES]
const NONE = 'none'

/** The most common value, ignoring unknowns. */
function mostCommon(values: string[]): string | null {
  return sharedDomain(values)
}

const rowName = (t: TrackRow) => t.name ?? t.callsign ?? (t.gold_name !== 'UNKNOWN' ? t.gold_name : t.track_id)

/**
 * A group a track manager forms from tracks (a battle group, a flight, a convoy), in a right
 * drawer. It is published as a track of its own at its members' centre, with the symbol built
 * here: a naval task organisation or the members' own function, the task-force indicator and the
 * echelon. With `groupId` it edits that group; without one it forms a new group of `members`.
 */
export function GroupEditor({
  groupId,
  members: initialMembers,
  rows,
  open,
  onClose,
  onSaved,
}: {
  groupId: string | null
  members: string[]
  rows: TrackRow[]
  open: boolean
  onClose: () => void
  onSaved: (groupId: string | null) => void
}) {
  const canManage = useCan('track_manager')
  const { toast, confirm } = useToast()
  const [group, setGroup] = useState<TrackGroup | null>(null)
  const [spec, setSpec] = useState<GroupSpec | null>(null)
  const [members, setMembers] = useState<string[]>([])
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)
  const byId = useMemo(() => new Map(rows.map((r) => [r.track_id, r])), [rows])

  useEffect(() => {
    if (!open) return
    setError(null)
    setSaved(false)
    if (groupId) {
      api.groups().then(
        (gs) => {
          const g = gs.find((x) => x.track_id === groupId) ?? null
          setGroup(g)
          setSpec(g?.spec ?? null)
          setMembers(g?.members ?? [])
          if (!g) setError(`${groupId} is no longer a group.`)
        },
        (e) => setError(errorMessage(e)),
      )
    } else {
      const ms = initialMembers.map((m) => byId.get(m)).filter((r): r is TrackRow => !!r)
      const domain = mostCommon(ms.map((m) => m.domain))
      const affiliation = mostCommon(ms.map((m) => m.affiliation)) ?? 'unknown'
      const base = domain === 'surface' ? 'task_group' : MEMBERS_BASE
      setGroup(null)
      setMembers(initialMembers)
      setSpec({ name: '', sidc: '', domain, affiliation, base, echelon: null, task_force: true, class: domain === 'surface' ? 'TASK GROUP' : 'GROUP' })
    }
    // The members are read when the drawer opens; live rows refreshing must not reset the form.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groupId, open])

  const memberRows = members.map((m) => byId.get(m))
  const derived = useMemo(
    () =>
      spec
        ? groupSidc({
            affiliation: spec.affiliation,
            domain: spec.domain,
            base: spec.base ?? MEMBERS_BASE,
            memberSidcs: memberRows.map((r) => (r ? (drawableSidc(r.sidc) ?? '') : '')),
            echelon: spec.echelon,
            taskForce: spec.task_force ?? false,
          })
        : '',
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [spec, members.join(',')],
  )
  const sidc = spec?.sidc_override ? spec.sidc : derived
  const preview = sidc ? symbolUrl({ standard: '2525c', code: sidc } as TrackRow['sidc'], 48) : null
  const baseline = group ? JSON.stringify([group.spec, group.members]) : null
  const current = spec ? JSON.stringify([{ ...spec, sidc }, members]) : null
  const dirty = !!spec && (baseline === null || current !== baseline)

  if (!spec) {
    return (
      <DetailDrawer open={open} onClose={onClose} label="Group editor" storageKey="ot.groupDetail.width" title={groupId ?? 'Group'}>
        <div className="panel-body">{error ? <div className="error-text">{error}</div> : <span className="muted">LOADING…</span>}</div>
      </DetailDrawer>
    )
  }
  const set = (patch: Partial<GroupSpec>) => {
    setSaved(false)
    setSpec({ ...spec, ...patch })
  }

  const save = async () => {
    setSaving(true)
    setError(null)
    try {
      const out: GroupSpec = { ...spec, sidc }
      if (!out.name.trim()) throw new Error('Name the group.')
      if (groupId && group) {
        await api.updateGroup(groupId, out)
        const add = members.filter((m) => !group.members.includes(m))
        const remove = group.members.filter((m) => !members.includes(m))
        if (add.length || remove.length) await api.groupMembers(groupId, add, remove)
        setGroup({ ...group, spec: out, members })
        onSaved(groupId)
      } else {
        if (members.length === 0) throw new Error('A group needs at least one member.')
        const r = await api.createGroup(out, members)
        toast({ variant: 'success', title: 'Group formed', message: `${out.name} is ${r.group}` })
        onSaved(r.group)
      }
      setSpec(out)
      setSaved(true)
    } catch (e) {
      setError(errorMessage(e))
    } finally {
      setSaving(false)
    }
  }

  const dissolve = async () => {
    if (!groupId) return
    const ok = await confirm(`Dissolve ${spec.name}? Its members stay; the group track is deleted downstream.`, {
      title: 'Dissolve group',
      confirmLabel: 'Dissolve',
    })
    if (!ok) return
    try {
      await api.dissolveGroup(groupId)
      onSaved(null)
      onClose()
    } catch (e) {
      setError(errorMessage(e))
    }
  }

  const row = (label: string, control: React.ReactNode, info?: string) => (
    <div className="entity-form-row">
      <label className="entity-form-label">
        {label}
        {info && <InfoTip label={label}>{info}</InfoTip>}
      </label>
      {control}
    </div>
  )

  return (
    <DetailDrawer
      open={open}
      onClose={onClose}
      label="Group editor"
      storageKey="ot.groupDetail.width"
      title={spec.name || (groupId ?? 'New group')}
      status={
        <Badge color={groupId ? 'grey' : 'blue'} size="sm" uppercase>
          {groupId ? 'group' : 'new group'}
        </Badge>
      }
      actions={
        <>
          {groupId && canManage && <Button size="sm" variant="ghost" icon={<TbTrash />} aria-label="Dissolve group" title="Dissolve group" onClick={dissolve} />}
          {canManage && <SaveButton size="sm" dirty={dirty} saving={saving} saved={saved} onSave={save} />}
        </>
      }
    >
      <div className="panel-body entity-editor">
        {groupId && <span className="muted mono">{groupId}</span>}
        {error && <div className="error-text">{error}</div>}
        <div className="group-head">
          {preview ? <img src={preview} alt="Group symbol" height={48} /> : <span className="muted">no symbol</span>}
          <div className="stack" style={{ gap: 2 }}>
            <span className="mono">{sidc}</span>
            <span className="muted">MIL-STD-2525C</span>
          </div>
        </div>

        <h4 className="subhead">
          Group
          <InfoTip label="Group">
            Published as a track of its own at the centre of its live members, with their mean course and speed and an uncertainty that
            covers them all. Members stay published and list the group.
          </InfoTip>
        </h4>
        <div className="entity-form">
          {row(
            'Name',
            <Input style={{ ...INPUT, width: '100%' }} aria-label="Group name" value={spec.name} placeholder="CSG 12" onChange={(e) => set({ name: e.target.value })} autoComplete="off" />,
          )}
          {row(
            'Class',
            <Input
              style={{ ...INPUT, width: '100%' }}
              aria-label="Group class"
              value={spec.class ?? ''}
              placeholder="CARRIER STRIKE GROUP"
              onChange={(e) => set({ class: e.target.value })}
              autoComplete="off"
            />,
            'Published as the OTH-GOLD class-name, e.g. CARRIER STRIKE GROUP, BOMBER FLIGHT, CONVOY.',
          )}
          {row(
            'Domain',
            <FieldSelect ariaLabel="Group domain" allowNone fields={DOMAINS.map((name) => ({ name }))} value={spec.domain ?? null} onChange={(v) => set({ domain: v })} style={{ width: '100%' }} />,
            "The group track's domain, for its force code and symbol. Starts as the members' most common known domain; left empty, the server uses that.",
          )}
          {row(
            'Affiliation',
            <FieldSelect
              ariaLabel="Group affiliation"
              fields={AFFILIATIONS.map((name) => ({ name }))}
              value={spec.affiliation ?? 'unknown'}
              onChange={(v) => set({ affiliation: v ?? 'unknown' })}
              style={{ width: '100%' }}
            />,
            "The group track's affiliation, for its force code and symbol colour. Starts as the members' most common known affiliation; it does not change the members'.",
          )}
        </div>

        <h4 className="subhead">
          Symbol
          <InfoTip label="Symbol">
            A naval task organisation icon (task force, group, unit, element, convoy), or the function the members share, so a flight of
            bombers keeps the bomber icon. The task-force indicator draws the bracket; the echelon draws its amplifier above the frame.
          </InfoTip>
        </h4>
        <div className="entity-form">
          {row(
            'Base',
            <FieldSelect
              ariaLabel="Symbol base"
              fields={BASES.map((b) => ({ name: b.name }))}
              value={BASES.find((b) => b.id === (spec.base ?? MEMBERS_BASE))?.name ?? null}
              onChange={(v) => {
                const b = BASES.find((x) => x.name === v) ?? BASES[0]
                const naval = NAVAL_BASES.find((n) => n.id === b.id)
                set({ base: b.id, ...(naval ? { domain: 'surface', class: spec.class || naval.className } : {}) })
              }}
              style={{ width: '100%' }}
            />,
          )}
          {row(
            'Echelon',
            <FieldSelect
              ariaLabel="Echelon"
              fields={[{ name: NONE }, ...ECHELONS.map((e) => ({ name: e.name }))]}
              value={ECHELONS.find((e) => e.letter === spec.echelon)?.name ?? NONE}
              onChange={(v) => set({ echelon: ECHELONS.find((e) => e.name === v)?.letter ?? null })}
              style={{ width: '100%' }}
            />,
          )}
          {row('Task force', <Toggle size="sm" aria-label="Task force" value={spec.task_force ?? false} onChange={(task_force) => set({ task_force })} />)}
          {row(
            'SIDC',
            <div className="num-row">
              <Input
                style={{ ...INPUT, width: 190, fontFamily: 'var(--font-mono)' }}
                aria-label="SIDC"
                value={sidc}
                disabled={!spec.sidc_override}
                onChange={(e) => set({ sidc: e.target.value.toUpperCase() })}
                spellCheck={false}
              />
              <Toggle size="sm" aria-label="Set the SIDC by hand" value={spec.sidc_override ?? false} onChange={(o) => set({ sidc_override: o, sidc })} />
              <span className="muted">by hand</span>
            </div>,
            'Built from the parts above. Turn on "by hand" to type any 2525C, 2525D or CoT code instead.',
          )}
        </div>

        <h4 className="subhead">
          Members ({members.length})
          <InfoTip label="Members">Add tracks with Group in the tracks table; remove them here.</InfoTip>
        </h4>
        <div className="group-members">
          {members.map((m, i) => {
            const r = memberRows[i]
            const icon = r ? symbolUrl(r.sidc, 20) : null
            return (
              <div key={m} className="group-member">
                {icon ? <img src={icon} alt="" height={20} /> : <span />}
                <span>{r ? rowName(r) : <span className="muted">not live</span>}</span>
                <span className="mono muted">{m}</span>
                <Button size="xs" variant="ghost" icon={<TbX />} aria-label={`Remove ${m}`} onClick={() => setMembers(members.filter((x) => x !== m))} />
              </div>
            )
          })}
          {members.length === 0 && <span className="muted">No members.</span>}
        </div>
      </div>
    </DetailDrawer>
  )
}
