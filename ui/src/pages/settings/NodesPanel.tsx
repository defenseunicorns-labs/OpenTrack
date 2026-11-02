import { useCallback, useEffect, useState } from 'react'
import { TbCopy, TbKey, TbPlus, TbTrash } from 'react-icons/tb'
import { Badge, Button, CollapsiblePanel, DataTable, Input, Modal, SaveButton, SegmentedControl, Toggle, useToast, type DataTableColumn } from '@kineticlogic/staresdk'
import { api, DEFAULT_SYNC, type PinnedKey, type SyncKeys, type SyncSettings, type SyncStatus } from '../../api/client'
import { useCan } from '../../auth/context'
import { InfoTip } from '../../components/InfoTip'
import { copyText } from '../../lib/clipboard'
import { looksLikeFingerprint } from '../../lib/syncKey'
import { acceptance, describeSync, DIRECTIONS, syncWarnings } from '../../lib/syncDirection'
import { errorMessage, fmtTime } from '../../lib/format'
import { INPUT } from '../../lib/valueSpec'
import { SettingsRow as Row } from './SettingsRow'

const REFRESH_MS = 5_000
/** A peer not heard for longer than this is shown as silent. */
const SILENT_MS = 30_000

interface PeerRow {
  site: string
  heard: string | null
  seq: number | null
  key: PinnedKey | null
}

/**
 * Sharing the picture with other OpenTrack nodes (docs/multi-node.md): which nodes this one trusts, whether it
 * accepts track management, and how the link is going. The settings are part of the page's draft.
 */
export function NodesPanel({
  value,
  onChange,
  siteCode,
  dirty,
  saving,
  saved,
  onSave,
}: {
  value: SyncSettings | undefined
  onChange: (s: SyncSettings) => void
  siteCode: string
  dirty: boolean
  saving: boolean
  saved: boolean
  onSave: () => void
}) {
  const sync: SyncSettings = { ...DEFAULT_SYNC, ...value }
  const set = (patch: Partial<SyncSettings>) => onChange({ ...sync, ...patch })
  const [peersText, setPeersText] = useState(sync.peers.join(', '))
  const [status, setStatus] = useState<SyncStatus | null>(null)
  useEffect(() => setPeersText(sync.peers.join(', ')), [sync.peers])
  useEffect(() => {
    const load = () => api.syncStatus().then(setStatus, () => {})
    load()
    const t = setInterval(load, REFRESH_MS)
    return () => clearInterval(t)
  }, [])
  const admin = useCan('admin')
  const { toast } = useToast()
  const [keys, setKeys] = useState<SyncKeys | null>(null)
  const [pinning, setPinning] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const loadKeys = useCallback(() => api.syncKeys().then(setKeys, () => {}), [])
  useEffect(() => void loadKeys(), [loadKeys])
  const copyKey = () =>
    copyText(keys?.public_key ?? '').then(
      () => toast({ variant: 'success', title: 'Copied', message: "This node's public key is on the clipboard." }),
      () => toast({ variant: 'error', title: 'Not copied', message: 'Select the public key above and copy it.' }),
    )
  const removeKey = async (site: string) => {
    if (!window.confirm(`Remove the key pinned for ${site}? Its messages are refused until a key is pinned again.`)) return
    try {
      setKeys(await api.removeSyncKey(site))
    } catch (err) {
      toast({ variant: 'error', title: 'Key not removed', message: errorMessage(err) })
    }
  }

  const heads = new Map((status?.log.heads ?? []).map((h) => [h.site, h.seq]))
  const heard = status?.link?.heard ?? {}
  const pinned = new Map((keys?.peers ?? []).map((k) => [k.site, k]))
  const rows: PeerRow[] = sync.peers.map((site) => ({ site, heard: heard[site] ?? null, seq: heads.get(site) ?? null, key: pinned.get(site) ?? null }))
  const unkeyed = rows.filter((r) => r.key == null).map((r) => r.site)
  const receiving = sync.direction !== 'send'
  const sending = sync.direction !== 'receive'
  const summary = describeSync(siteCode, sync, (site) => pinned.has(site))
  const warnings = syncWarnings(sync)
  const counts = status?.link?.counts ?? {}
  const columns: DataTableColumn<PeerRow>[] = [
    { key: 'site', header: 'Node', width: 90, mono: true, render: (r) => r.site },
    {
      key: 'state',
      header: 'Link',
      width: 110,
      render: (r) => {
        const live = r.heard != null && Date.now() - Date.parse(r.heard) < SILENT_MS
        return (
          <Badge color={live ? 'success' : r.heard == null ? 'grey' : 'warning'} size="sm">
            {r.heard == null ? 'never heard' : live ? 'heard' : 'silent'}
          </Badge>
        )
      },
    },
    {
      key: 'accepts',
      header: 'Taken in',
      width: 120,
      render: (r) => {
        const a = acceptance(sync, r.key != null)
        return (
          <Badge color={a.ok ? 'success' : r.key == null ? 'danger' : 'grey'} size="sm">
            {a.text}
          </Badge>
        )
      },
    },
    { key: 'heard', header: 'Last heard', width: 180, mono: true, render: (r) => (r.heard ? fmtTime(Date.parse(r.heard)) : '—') },
    { key: 'seq', header: 'Its decisions held', width: 140, mono: true, render: (r) => r.seq ?? 0 },
    {
      key: 'key',
      header: 'Key',
      render: (r) =>
        r.key ? (
          <span className="mono" title={`${r.key.public_key}\npinned by ${r.key.pinned_by}${r.key.pinned_at ? ` at ${fmtTime(Date.parse(r.key.pinned_at))}` : ''}`}>
            {r.key.fingerprint ?? 'not a key'}
          </span>
        ) : (
          <Badge color="danger" size="sm">
            no key: refused
          </Badge>
        ),
    },
    ...(admin
      ? [
          {
            key: 'actions',
            header: '',
            width: 150,
            render: (r: PeerRow) => (
              <div className="num-row">
                <Button size="sm" variant="ghost" icon={<TbKey />} onClick={() => setPinning(r.site)}>
                  {r.key ? 'Replace' : 'Pin key'}
                </Button>
                {r.key && <Button size="sm" variant="ghost" icon={<TbTrash />} aria-label={`Remove the key pinned for ${r.site}`} onClick={() => removeKey(r.site)} />}
              </div>
            ),
          },
        ]
      : []),
  ]
  const byStatus = status?.log.by_status ?? {}
  const e = status?.engine

  return (
    <CollapsiblePanel title="Nodes" persistKey="ot.panel.settings.nodes" actions={<SaveButton size="sm" dirty={dirty} saving={saving} saved={saved} onSave={onSave} />}>
      <div className="panel-body stack">
        <div className="node-summary" role="status" aria-label="What this node does">
          <strong>{summary}</strong>
          {warnings.map((w) => (
            <div key={w} className="num-row">
              <Badge color="warning" size="sm">
                note
              </Badge>
              <span>{w}</span>
            </div>
          ))}
        </div>
        <Row label="Share the picture" hint="Exchange tracks and track-management decisions with other nodes, through this node's NATS (ot.sync.*), which a networking package or opentrack bridge carries. Every node gets the whole picture under one set of track numbers; each reports only what its own sensors see.">
          <Toggle value={sync.enabled} onChange={(enabled) => set({ enabled })} aria-label="Share the picture" />
        </Row>
        <Row
          label="Direction"
          hint="Send and receive: share this node's tracks and decisions and take in the trusted nodes'. Receive only: take in the trusted nodes' tracks and decisions and send nothing at all; nothing about this node goes over the link. Send only: share this node's picture and take nothing in."
        >
          <SegmentedControl
            aria-label="Direction"
            size="sm"
            value={sync.direction}
            onChange={(direction) => set({ direction: direction as SyncSettings['direction'] })}
            options={DIRECTIONS}
          />
        </Row>
        {sync.direction === 'receive' && (
          <Row
            label="Ask peers to fill gaps"
            hint="Off: this node sends nothing at all, and a decision lost on the link is never asked for again (tracks recover on their own: the next report replaces a lost one). On: it also sends small summaries and requests for what it missed, which carry no tracks and no decisions."
          >
            <Toggle value={sync.repair} onChange={(repair) => set({ repair })} aria-label="Ask peers to fill gaps" />
          </Row>
        )}
        <Row
          label="Trusted nodes"
          hint={
            receiving
              ? `Site codes of the nodes whose tracks and decisions this node takes in, comma separated (this node is ${siteCode}). Messages from any other node are dropped, and so are a trusted node's until its public key is pinned in the table below.`
              : `Sending only, this node takes nothing in and needs no trusted nodes. A trusted node with a pinned key has its requests for what it missed answered.`
          }
        >
          <Input
            style={{ ...INPUT, width: 320 }}
            aria-label="Trusted nodes"
            placeholder="e.g. AAA, BBB"
            value={peersText}
            onChange={(ev) => setPeersText(ev.target.value)}
            onBlur={() =>
              set({
                peers: peersText
                  .split(/[\s,]+/)
                  .map((p) => p.trim().toUpperCase())
                  .filter(Boolean),
              })
            }
          />
        </Row>
        {sending && (
          <Row label="Sending budget" hint="What this node may send to the others for its tracks, kbit/s: its share of the link (0: no cap). Past it, new tracks, state changes and the largest drifts go first and the rest wait. A track costs about 40 bytes a report; a steady one is reported every 12 s.">
            <div className="num-row">
              <Input
                style={{ ...INPUT, width: 90 }}
                type="number"
                min={0}
                step="any"
                aria-label="Sending budget kbit/s"
                value={sync.budget_kbps ?? 0}
                onChange={(ev) => set({ budget_kbps: ev.target.value === '' ? 0 : Number(ev.target.value) })}
              />
              <span className="muted">kbit/s</span>
            </div>
          </Row>
        )}
        <Row
          label="This node's key"
          hint="Every sync message this node sends is signed with this key. Give the public key (the ed25519: line) to each other node's admin to pin; compare the fingerprint over another channel (a phone call, the deployment plan)."
        >
          <div className="node-key">
            <div className="num-row">
              <span className="muted">Public key</span>
              <code className="mono node-key-text" aria-label="This node's public key">
                {keys?.public_key ?? '—'}
              </code>
              <Button size="sm" variant="ghost" icon={<TbCopy />} onClick={copyKey} disabled={!keys}>
                Copy
              </Button>
            </div>
            <div className="num-row">
              <span className="muted">Fingerprint</span>
              <span className="mono">{keys?.fingerprint ?? '—'}</span>
            </div>
          </div>
        </Row>
        <Row
          label="Accept no track management here"
          hint="Apply other nodes' track management here, but refuse it from this node's own users: for a node with no operator, such as a drone. It does not change what this node sends; that is the Direction."
        >
          <Toggle value={sync.no_local_management} onChange={(no_local_management) => set({ no_local_management })} aria-label="Accept no track management here" />
        </Row>
        <Row label="Share the profile" hint="Publishing an output schema or saving correlation settings on any node applies on every node that shares the profile (the later change wins), so every node publishes the same attributes and pairs by the same rules.">
          <Toggle value={sync.share_profile} onChange={(share_profile) => set({ share_profile })} aria-label="Share the profile" />
        </Row>
        {(sync.peers.length > 0 || admin) && (
          <>
            <h4 className="subhead with-action">
              Trusted nodes
              <InfoTip label="Trusted nodes">
                Taken in: whether this node takes in the node&apos;s tracks and decisions (it needs a pinned key, and this node must not be sending
                only). Link: heard if a message from the node arrived in the last 30 s, silent if longer ago, never heard if none since this node
                started. Its decisions held: the highest sequence number of that node&apos;s track-management decisions stored here (AAA:12
                means 12); gaps below it are asked for again.
              </InfoTip>
              {admin && (
                <Button size="sm" variant="ghost" icon={<TbPlus />} onClick={() => setAdding(true)}>
                  Add a node
                </Button>
              )}
            </h4>
            <DataTable aria-label="Trusted nodes" rows={rows} columns={columns} rowKey={(r) => r.site} density="compact" />
            {unkeyed.length > 0 && (
              <div className="num-row">
                <Badge color="danger" size="sm">
                  {unkeyed.length === 1 ? `${unkeyed[0]} has no key` : `${unkeyed.length} nodes have no key`}
                </Badge>
                <span className="muted">Messages from a node without a pinned key are refused.</span>
              </div>
            )}
          </>
        )}
        <div className="num-row muted">
          <span>This node reports {e?.reporting ?? 0} tracks to the others and holds {e?.from_other_nodes ?? 0} only they see.</span>
          <span>
            Decisions: {byStatus.applied ?? 0} applied, {byStatus.pending ?? 0} waiting for their tracks, {byStatus.superseded ?? 0} superseded,{' '}
            {byStatus.failed ?? 0} failed.
          </span>
          <InfoTip label="Decisions">
            Track management from every node, under ids like AAA:12. One naming a track this node has not heard of yet waits for it (12 h at most). One that
            conflicts with a later decision on the same tracks is kept but changes nothing.
          </InfoTip>
          {status?.link === undefined || status?.link === null ? <Badge color="grey" size="sm">no link running</Badge> : null}
        </div>
        {(counts.not_sent ?? 0) + (counts.not_received ?? 0) > 0 && (
          <div className="num-row muted">
            <span>
              Held back by the direction since the link started: {counts.not_sent ?? 0} not sent, {counts.not_received ?? 0} not taken in.
            </span>
          </div>
        )}
        {(counts.no_key ?? 0) + (counts.bad_signature ?? 0) + (counts.stale ?? 0) + (counts.replayed ?? 0) + (counts.untrusted ?? 0) > 0 && (
          <div className="num-row muted">
            <span>
              Refused since the link started: {counts.untrusted ?? 0} from untrusted nodes, {counts.no_key ?? 0} with no key pinned,{' '}
              {counts.bad_signature ?? 0} failing their signature, {counts.stale ?? 0} too old, {counts.replayed ?? 0} repeated.
            </span>
            <InfoTip label="Refused messages">
              A message is accepted only from a trusted node with a pinned key, when its signature verifies with that key, its clock is within 5 minutes of
              this node&apos;s, and it has not been accepted before. A failing signature from a trusted node may be another node using its name: it is
              logged and recorded in the audit log. After replacing a node&apos;s key, pin the new one here.
            </InfoTip>
          </div>
        )}
        {adding && (
          <AddNodeModal
            siteCode={siteCode}
            peers={sync.peers}
            keyOptional={!receiving}
            onClose={() => setAdding(false)}
            onAdded={(site, k) => {
              if (k) setKeys(k)
              if (!sync.peers.includes(site)) set({ peers: [...sync.peers, site] })
              setAdding(false)
              toast({ variant: 'success', title: `${site} added`, message: 'Save the settings to start taking it in.' })
            }}
          />
        )}
        {pinning && (
          <PinKeyModal
            site={pinning}
            current={pinned.get(pinning) ?? null}
            onClose={() => setPinning(null)}
            onPinned={(k) => {
              setKeys(k)
              setPinning(null)
            }}
          />
        )}
      </div>
    </CollapsiblePanel>
  )
}

/** Pin (or replace) the public key of a peer: an admin's decision, recorded in the decision and audit logs. */
function PinKeyModal({ site, current, onClose, onPinned }: { site: string; current: PinnedKey | null; onClose: () => void; onPinned: (k: SyncKeys) => void }) {
  const { toast } = useToast()
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const fingerprint = looksLikeFingerprint(text)
  const save = async () => {
    setBusy(true)
    try {
      onPinned(await api.pinSyncKey(site, text.trim()))
    } catch (err) {
      toast({ variant: 'error', title: 'Key not pinned', message: errorMessage(err) })
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal title={`${current ? 'Replace' : 'Pin'} the key of ${site}`} onClose={onClose} width={560} resizable={false}>
      <div className="panel-body stack">
        <div className="num-row">
          <span>Paste {site}&apos;s public key (ed25519:…), from its Settings → Nodes.</span>
          <InfoTip label="Pinning a key">
            Compare the fingerprint shown after pinning with the one {site}&apos;s admin reads out over another channel. From then on this node accepts sync
            messages claiming to be from {site} only if they are signed with this key.
          </InfoTip>
        </div>
        {current && <span className="muted mono">Now: {current.fingerprint ?? current.public_key}</span>}
        <Input aria-label={`Public key of ${site}`} placeholder="ed25519:…" value={text} onChange={(ev) => setText(ev.target.value)} />
        {fingerprint && (
          <div className="error-text">
            That is {site}&apos;s fingerprint, not its key. Copy the public key (the ed25519: line) from {site}&apos;s Settings → Nodes.
          </div>
        )}
        <div className="num-row" style={{ justifyContent: 'flex-end' }}>
          <Button size="sm" variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" onClick={save} disabled={busy || text.trim() === '' || fingerprint}>
            Pin
          </Button>
        </div>
      </div>
    </Modal>
  )
}

/**
 * Add a trusted node in one step: its site code and its public key, pinned at once (an admin's decision, audited);
 * the fingerprint is shown to compare with the other node's admin before the node joins the draft.
 */
function AddNodeModal({
  siteCode,
  peers,
  keyOptional,
  onClose,
  onAdded,
}: {
  siteCode: string
  peers: string[]
  keyOptional: boolean
  onClose: () => void
  onAdded: (site: string, keys: SyncKeys | null) => void
}) {
  const { toast } = useToast()
  const [site, setSite] = useState('')
  const [key, setKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [pinned, setPinned] = useState<SyncKeys | null>(null)
  const code = site.trim().toUpperCase()
  const siteError =
    code === ''
      ? null
      : !/^[A-Z0-9]{3}$/.test(code)
        ? 'A site code is 3 characters, A-Z and 0-9.'
        : code === siteCode
          ? `${code} is this node's own site code.`
          : peers.includes(code)
            ? `${code} is trusted already: pin or replace its key in the table.`
            : null
  const fingerprint = looksLikeFingerprint(key)
  const fp = pinned?.peers.find((p) => p.site === code)?.fingerprint ?? null
  const ready = code !== '' && siteError == null && !fingerprint && (keyOptional || key.trim() !== '')
  const add = async () => {
    if (key.trim() === '') {
      onAdded(code, null)
      return
    }
    setBusy(true)
    try {
      setPinned(await api.pinSyncKey(code, key.trim()))
    } catch (err) {
      toast({ variant: 'error', title: 'Key not pinned', message: errorMessage(err) })
    } finally {
      setBusy(false)
    }
  }
  return (
    <Modal title="Add a node" onClose={onClose} width={560} resizable={false}>
      <div className="panel-body stack">
        {pinned ? (
          <>
            <span>
              {code}&apos;s key is pinned. Its fingerprint here is:
            </span>
            <code className="mono" aria-label={`Fingerprint of ${code}`}>
              {fp ?? '—'}
            </code>
            <span className="muted">
              Compare it with the fingerprint {code}&apos;s admin reads out from {code}&apos;s Settings → Nodes, over another channel. If they differ, remove
              the key.
            </span>
            <div className="num-row" style={{ justifyContent: 'flex-end' }}>
              <Button size="sm" onClick={() => onAdded(code, pinned)}>
                They match: add {code}
              </Button>
            </div>
          </>
        ) : (
          <>
            <Input aria-label="Site code of the node" placeholder="Site code, e.g. AAA" value={site} maxLength={3} onChange={(ev) => setSite(ev.target.value)} />
            {siteError && <div className="error-text">{siteError}</div>}
            <Input aria-label="Public key of the node" placeholder="ed25519:…" value={key} onChange={(ev) => setKey(ev.target.value)} />
            <span className="muted">
              {keyOptional
                ? 'Its public key (the ed25519: line from its Settings → Nodes). This node sends only, so the key is needed only to answer its requests for what it missed.'
                : 'Its public key: the ed25519: line from its Settings → Nodes. Its messages are refused until a key is pinned.'}
            </span>
            {fingerprint && (
              <div className="error-text">That is a fingerprint, not a key. Copy the public key (the ed25519: line) from the other node&apos;s Settings → Nodes.</div>
            )}
            <div className="num-row" style={{ justifyContent: 'flex-end' }}>
              <Button size="sm" variant="ghost" onClick={onClose}>
                Cancel
              </Button>
              <Button size="sm" onClick={add} disabled={busy || !ready}>
                {key.trim() === '' ? 'Add' : 'Pin the key'}
              </Button>
            </div>
          </>
        )}
      </div>
    </Modal>
  )
}
