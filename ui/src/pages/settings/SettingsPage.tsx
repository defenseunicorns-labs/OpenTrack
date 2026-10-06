import { useEffect, useState } from 'react'
import { TbDownload, TbTrash, TbUpload } from 'react-icons/tb'
import { Badge, Button, CollapsiblePanel, FileDropZone, Input, Label, Modal, SaveButton, Toggle, useToast } from '@kineticlogic/staresdk'
import { api, type AppSettings, type AppSettingsResponse, type ConfigImportStatus } from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { NodesPanel } from './NodesPanel'
import { PluginsSection } from './PluginsPanel'
import { SecurityPanel } from './SecurityPanel'
import { TakPanel } from './TakPanel'
import { UsersPanel } from './UsersPanel'
import { useAuth, useCan } from '../../auth/context'
import { errorMessage } from '../../lib/format'
import { INPUT } from '../../lib/valueSpec'

/** OpenStare's classification presets (background, text colour). */
const PRESETS = [
  { label: 'Unclassified', background: '#006400', color: '#ffffff' },
  { label: 'CUI', background: '#502b85', color: '#ffffff' },
  { label: 'Confidential', background: '#0033a0', color: '#ffffff' },
  { label: 'Secret', background: '#c8102e', color: '#ffffff' },
  { label: 'Top Secret', background: '#ff8300', color: '#000000' },
]

const SITE_CODE_INFO =
  'Site code: the 3 characters (A–Z, 0–9) that begin every track number this instance issues, e.g. OTK000000042 — ' +
  "OTH-GOLD's track UID form, a site code then a 9-digit sequence. It keeps track numbers unique between the sites feeding one " +
  'picture, so every OpenTrack needs its own. It is set at deployment (OT_SITE_CODE) and cannot change here: published track numbers would change with it.'

const CONFIG_EXPORT_INFO =
  "The whole configuration, to back this node up or rebuild it: sources (with their credentials), output schema versions, " +
  'correlation, instance and sign-in settings, accounts with their password hashes, API token records, the registry, plugins, ' +
  'imported tracker profiles and the track number counter. It holds secrets and password hashes: keep it as safe as the ' +
  'database and delete copies you no longer need. The session signing key, sessions, the audit record and track state are ' +
  'never in it, so API tokens work again only on a node with the same key. Admins only; each export is audited.'

const CONFIG_IMPORT_INFO =
  'Rebuild this node from a full configuration export. Offered only while this node has no configuration (no sources, ' +
  'registry, plugins or saved settings, no account but its first admin, and no earlier import). Everything is imported or nothing is. The ' +
  'imported accounts replace the first admin, so you sign in again with one of them.'

const BASEMAP_HINT =
  'An XYZ tile URL, such as https://tiles.example/{z}/{x}/{y}.png. OpenTrack fetches the tiles for the browser. Empty: the built-in country outlines.'

function Row({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div className="settings-row">
      <div className="row-label">
        <Label size="sm">{label}</Label>
        {hint && <InfoTip label={label}>{hint}</InfoTip>}
      </div>
      <div className="settings-row-control">{children}</div>
    </div>
  )
}

/** Instance settings: site name, banners, plugins, users and security (admins), data export, purge. */
export default function SettingsPage({ onSaved }: { onSaved: () => void }) {
  const { toast, confirm } = useToast()
  const [loaded, setLoaded] = useState<AppSettingsResponse | null>(null)
  const [draft, setDraft] = useState<AppSettings | null>(null)
  const [purgeOpen, setPurgeOpen] = useState(false)
  const [saving, setSaving] = useState(false)
  const [saved, setSaved] = useState(false)
  const admin = useCan('admin')
  const { user } = useAuth()
  // An admin on an address that doesn't serve the admin routes (OT_ADMIN_BIND).
  const adminElsewhere = user?.role === 'admin' && user.admin_api === false
  const [importStatus, setImportStatus] = useState<ConfigImportStatus | null>(null)
  const [importFile, setImportFile] = useState<File | null>(null)
  const [importing, setImporting] = useState(false)

  useEffect(() => {
    if (!admin) return
    api.configImportStatus().then(setImportStatus, () => setImportStatus(null))
  }, [admin])

  useEffect(() => {
    api.appSettings().then(
      (r) => {
        setLoaded(r)
        setDraft(r.settings)
      },
      (e) => toast({ variant: 'error', title: 'Settings', message: errorMessage(e) }),
    )
  }, [toast])

  if (!draft || !loaded) return <span className="muted">LOADING…</span>
  const b = draft.banner
  const setB = (patch: Partial<AppSettings['banner']>) => setDraft({ ...draft, banner: { ...b, ...patch } })
  const w = draft.warning ?? { enabled: false, text: '' }
  const setW = (patch: Partial<AppSettings['warning']>) => setDraft({ ...draft, warning: { ...w, ...patch } })
  const dirty = JSON.stringify(draft) !== JSON.stringify(loaded.settings)
  const hours = draft.history_hours ?? 12
  const interval = draft.history_interval_secs ?? 10
  const perThousand = interval > 0 ? (1000 * hours * 3600 * 130) / interval : null
  const historyEstimate =
    perThousand == null || hours <= 0 ? null : perThousand >= 1e9 ? `${(perThousand / 1e9).toFixed(1)} GB` : `${Math.round(perThousand / 1e6)} MB`

  const save = async () => {
    setSaving(true)
    setSaved(false)
    try {
      const r = await api.saveAppSettings(draft)
      setLoaded(r)
      setDraft(r.settings)
      onSaved()
      setSaved(true)
    } catch (e) {
      toast({ variant: 'error', title: 'Not saved', message: errorMessage(e) })
    } finally {
      setSaving(false)
    }
  }

  const importConfig = async () => {
    if (!importFile) return
    const ok = await confirm(
      `Every source, setting, account, registry entity and plugin in ${importFile.name} is written into this node, and its accounts replace yours: you will sign in again with one of them.`,
      { title: 'Import the configuration', confirmLabel: 'Import' },
    )
    if (!ok) return
    setImporting(true)
    try {
      const r = await api.importConfig(importFile)
      toast({ variant: 'success', title: 'Configuration imported', message: r.notes.join(' ') })
      window.setTimeout(() => window.location.assign('/'), 2500)
    } catch (e) {
      toast({ variant: 'error', title: 'Not imported', message: errorMessage(e) })
      api.configImportStatus().then(setImportStatus, () => setImportStatus(null))
    } finally {
      setImporting(false)
    }
  }

  return (
    <div className="stack">
      <CollapsiblePanel title="General" persistKey="ot.panel.settings.instance"
        actions={
          admin ? (
            <SaveButton size="sm" dirty={dirty} saving={saving} saved={saved} onSave={save} />
          ) : (
            adminElsewhere && (
              <InfoTip label="Admin address">
                Admin changes and reads (accounts, security, sources, settings) are made on the admin address, not this one. Your administrator has it.
              </InfoTip>
            )
          )
        }
      >
        <div className="panel-body">
          <Row label="Site name" hint="A name for this instance, up to 64 characters, shown in the header and browser tab as OpenTrack · name. Display only: track numbers use the site code.">
            <div className="num-row">
              <Input style={{ ...INPUT, width: 260 }} aria-label="Site name" value={draft.site_name} maxLength={64} onChange={(e) => setDraft({ ...draft, site_name: e.target.value })} />
              <span className="muted">site code</span>
              <Badge color="grey" size="sm">
                {loaded.site_code}
              </Badge>
              <InfoTip label="Site code">{SITE_CODE_INFO}</InfoTip>
            </div>
          </Row>
          <Row label="Position history">
            <div className="num-row">
              <Input
                style={{ ...INPUT, width: 90 }}
                type="number"
                min={0}
                max={720}
                step="any"
                aria-label="History hours"
                placeholder="12"
                value={draft.history_hours ?? ''}
                onChange={(e) => setDraft({ ...draft, history_hours: e.target.value === '' ? null : Number(e.target.value) })}
              />
              <span className="muted">hours, a point every</span>
              <Input
                style={{ ...INPUT, width: 90 }}
                type="number"
                min={0}
                max={3600}
                step="any"
                aria-label="History interval seconds"
                placeholder="10"
                value={draft.history_interval_secs ?? ''}
                onChange={(e) => setDraft({ ...draft, history_interval_secs: e.target.value === '' ? null : Number(e.target.value) })}
              />
              <span className="muted">s</span>
              <InfoTip label="Position history">
                How long each track&apos;s positions are kept (0: none, at most 720; empty: 12 h), and at most one point per track this often (0: every
                update, at most 3600; empty: 10 s). Memory in Redis is about 130 bytes a point; points per track = hours × 3600 ÷ interval. For example
                2,000 tracks for 12 h every 10 s take about 1.1 GB; 16,000 tracks about 9 GB.
              </InfoTip>
              {historyEstimate && <span className="muted">≈ {historyEstimate} per 1,000 tracks</span>}
            </div>
          </Row>
          <Row label="Basemap tiles" hint={BASEMAP_HINT}>
            {admin ? (
              <Input
                style={{ ...INPUT, width: 420 }}
                aria-label="Basemap tiles URL"
                placeholder="https://tiles.example/{z}/{x}/{y}.png"
                value={draft.basemap_tiles_url ?? ''}
                maxLength={2048}
                onChange={(e) => setDraft({ ...draft, basemap_tiles_url: e.target.value })}
              />
            ) : (
              <Badge color={loaded.basemap_tiles ? 'success' : 'grey'} size="sm">
                {loaded.basemap_tiles ? 'on' : 'off'}
              </Badge>
            )}
          </Row>
        </div>
        <PluginsSection />
      </CollapsiblePanel>

      {admin && <UsersPanel />}

      {admin && (
        <NodesPanel
          value={draft.sync}
          onChange={(sync) => setDraft({ ...draft, sync })}
          siteCode={loaded.site_code}
          dirty={dirty}
          saving={saving}
          saved={saved}
          onSave={save}
        />
      )}
      {admin && <TakPanel value={draft.tak} onChange={(tak) => setDraft({ ...draft, tak })} dirty={dirty} saving={saving} saved={saved} onSave={save} />}

      <CollapsiblePanel title="Data" persistKey="ot.panel.settings.data">
        <div className="panel-body">
          <Row label="Live tracks" hint="Every live track as published: the GOLD fields, attributes, state, confidence and sources. Each file is marked with the highest security label in it (the banner's classification when none has one), and each labelled track with its own marking.">
            <div className="num-row">
              <Button size="sm" variant="ghost" icon={<TbDownload />} onClick={() => window.open(api.exportUrl('tracks.geojson'), '_self')}>
                GeoJSON
              </Button>
              <Button size="sm" variant="ghost" icon={<TbDownload />} onClick={() => window.open(api.exportUrl('tracks.csv'), '_self')}>
                CSV
              </Button>
            </div>
          </Row>
          <Row label="Full configuration" hint={CONFIG_EXPORT_INFO}>
            <Button size="sm" variant="ghost" disabled={!admin} icon={<TbDownload />} onClick={() => window.open(api.exportUrl('config'), '_self')}>
              Configuration
            </Button>
          </Row>
          {admin && importStatus?.empty && (
            <Row label="Import configuration" hint={CONFIG_IMPORT_INFO}>
              <div className="stack">
                <FileDropZone
                  inputId="config-import-file"
                  accept=".json,application/json"
                  acceptedExtensions={['.json']}
                  file={importFile}
                  onFileChange={setImportFile}
                  onReject={(m) => toast({ variant: 'error', title: 'Configuration', message: m })}
                  label="A full configuration export (.json)"
                  hint={`${importStatus.format} version ${importStatus.version}`}
                  compact
                />
                <div className="num-row">
                  <Button size="sm" variant="primary" icon={<TbUpload />} disabled={!importFile || importing} onClick={importConfig}>
                    Import
                  </Button>
                </div>
              </div>
            </Row>
          )}
          <Row label="Registry" hint="Entities with their identifiers and attributes: export and import them on the Registry tab.">
            <Button size="sm" variant="ghost" icon={<TbDownload />} onClick={() => window.open(api.registryExportUrl('xlsx'), '_self')}>
              Registry XLSX
            </Button>
          </Row>
          <h4 className="subhead">
            Purge
            <InfoTip label="Purge">
              Retire every live track, and delete the published ones in OpenStare. Sources, the output schema, the registry and the decision log stay.
            </InfoTip>
          </h4>
          <Row label="Purge tracks" hint="Retire every live track. You confirm, and choose whether history goes too, in the next step.">
            <Button size="sm" variant="danger" icon={<TbTrash />} disabled={!admin} onClick={() => setPurgeOpen(true)}>
              Purge tracks
            </Button>
          </Row>
        </div>
      </CollapsiblePanel>

      {/* As OpenStare's Banner settings: this instance's own marking, whatever OpenStare shows. */}
      <CollapsiblePanel title="Banners" persistKey="ot.panel.settings.banner" actions={admin && <SaveButton size="sm" dirty={dirty} saving={saving} saved={saved} onSave={save} />}>
        <div className="panel-body banner-settings">
          <div className="banner-toggle">
            <div className="field-caps">
              Classification banner
              <InfoTip label="Classification banner">Displays a fixed bar at the top and bottom of every page.</InfoTip>
            </div>
            <Toggle value={b.enabled} onChange={(enabled) => setB({ enabled })} aria-label="Classification Banner" />
          </div>
          <div>
            <div className="field-caps">
              Classification text
              <InfoTip label="Classification text">The marking shown in the banner, 1 to 128 characters (required while the banner is on).</InfoTip>
            </div>
            <Input style={{ width: '100%' }} value={b.text} maxLength={128} onChange={(e) => setB({ text: e.target.value })} placeholder="e.g. UNCLASSIFIED // FOR OFFICIAL USE ONLY" />
          </div>
          <div>
            <div className="field-caps">
              Color preset
              <InfoTip label="Color preset">Sets the background and text colours to the usual ones for that marking. Adjust them below if needed.</InfoTip>
            </div>
            <div className="banner-presets">
              {PRESETS.map((p) => (
                <button
                  key={p.label}
                  type="button"
                  className={b.background === p.background ? 'banner-preset selected' : 'banner-preset'}
                  style={{ background: p.background, color: p.color }}
                  onClick={() => setB({ background: p.background, color: p.color })}
                >
                  {p.label}
                </button>
              ))}
            </div>
          </div>
          <div className="banner-colours">
            {(
              [
                ['background', 'Background color', '#006400'],
                ['color', 'Text color', '#ffffff'],
              ] as const
            ).map(([key, label, placeholder]) => (
              <div key={key}>
                <div className="field-caps">
                  {label}
                  <InfoTip label={label}>Pick a colour or type it as #rrggbb.</InfoTip>
                </div>
                <div className="banner-colour">
                  <input type="color" aria-label={label} value={b[key]} onChange={(e) => setB({ [key]: e.target.value })} />
                  <Input style={{ flex: 1 }} aria-label={`${label} hex`} value={b[key]} onChange={(e) => setB({ [key]: e.target.value })} placeholder={placeholder} />
                </div>
              </div>
            ))}
          </div>
          <div>
            <div className="field-caps">Preview</div>
            <div className="banner-preview" style={{ background: b.background, color: b.color, opacity: b.enabled ? 1 : 0.35 }}>
              {b.text || 'UNCLASSIFIED'}
            </div>
          </div>
          <div className="banner-toggle">
            <div className="field-caps">
              Warning banner
              <InfoTip label="Warning banner">
                A notice users must accept after signing in, such as consent to monitoring. Declining signs them out. Asked once per browser session.
              </InfoTip>
            </div>
            <Toggle value={w.enabled} onChange={(enabled) => setW({ enabled })} aria-label="Warning Banner" />
          </div>
          <div>
            <div className="field-caps">
              Warning text
              <InfoTip label="Warning text">What users read and accept or decline, up to 20,000 characters. Line breaks are kept.</InfoTip>
            </div>
            <textarea
              className="plain-textarea"
              style={{ maxWidth: 'none', fontFamily: 'var(--font-sans)' }}
              aria-label="Warning text"
              rows={8}
              maxLength={20000}
              value={w.text}
              onChange={(e) => setW({ text: e.target.value })}
              placeholder="e.g. You are accessing a U.S. Government information system…"
            />
          </div>
        </div>
      </CollapsiblePanel>

      {admin && <SecurityPanel />}
      {purgeOpen && <PurgeModal siteCode={loaded.site_code} onClose={() => setPurgeOpen(false)} />}
    </div>
  )
}

/** Settings → Data → Purge: what a purge does, whether history goes too, and the one confirm. */
function PurgeModal({ siteCode, onClose }: { siteCode: string; onClose: () => void }) {
  const { toast } = useToast()
  const [history, setHistory] = useState(false)
  const [busy, setBusy] = useState(false)
  const purge = async () => {
    setBusy(true)
    try {
      // The API still asks for the site code, so a stray request can't purge; the modal is the confirmation.
      const r = await api.purge(siteCode, history)
      toast({ variant: 'success', title: 'Purged', message: `${r.retired} tracks retired${r.history ? `, ${r.history.nodes} graph nodes deleted` : ''}` })
      onClose()
    } catch (e) {
      toast({ variant: 'error', title: 'Not purged', message: errorMessage(e) })
      setBusy(false)
    }
  }
  return (
    <Modal title="Purge every track" onClose={onClose} width={520} resizable={false}>
      <div className="panel-body stack">
        <p className="purge-warning">
          Every live track is retired now, and the published ones are deleted in OpenStare too. Sources, the output schema, the registry and the decision log
          stay, and sources keep reporting, so new tracks will appear. This cannot be undone.
        </p>
        <Row label="Also delete history" hint="The track graph: how every track was formed, paired, merged and split.">
          <Toggle size="sm" aria-label="Also delete history" value={history} onChange={setHistory} />
        </Row>
        <div className="num-row" style={{ justifyContent: 'flex-end' }}>
          <Button size="sm" variant="ghost" type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" variant="danger" icon={<TbTrash />} disabled={busy} onClick={purge}>
            Purge
          </Button>
        </div>
      </div>
    </Modal>
  )
}
