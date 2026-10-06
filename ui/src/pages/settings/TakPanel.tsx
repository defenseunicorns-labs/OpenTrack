import { useEffect, useState } from 'react'
import { TbEdit, TbPlus, TbTrash } from 'react-icons/tb'
import { Badge, Button, CollapsiblePanel, DataTable, FieldSelect, Input, Modal, SaveButton, Toggle, type BadgeColor, type DataTableColumn } from '@kineticlogic/staresdk'
import { api, type TakOutput, type TakSettings, type TakStatus } from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { INPUT } from '../../lib/valueSpec'
import { TAK_KINDS, cleanOutput, isEncrypted, kindLabel, newDelivery, newOutput, outputProblem, outputWhere, type TakKind } from '../../lib/tak'
import { SettingsRow as Row } from './SettingsRow'

const REFRESH_MS = 5_000

const STATE_COLOR: Record<string, BadgeColor> = {
  connected: 'success',
  listening: 'success',
  sending: 'success',
  connecting: 'warning',
  disconnected: 'danger',
  error: 'danger',
  off: 'grey',
  'not started': 'grey',
}

type Status = TakStatus['outputs'][number]

/** A number input that holds what is typed until it is a number. */
function NumberInput({ label, value, onChange, width = 90, min, max }: { label: string; value: number; onChange: (n: number) => void; width?: number; min?: number; max?: number }) {
  return (
    <Input
      style={{ ...INPUT, width }}
      type="number"
      min={min}
      max={max}
      step="any"
      aria-label={label}
      value={Number.isFinite(value) ? value : ''}
      onChange={(e) => onChange(e.target.value === '' ? NaN : Number(e.target.value))}
    />
  )
}

function TextInput({ label, value, onChange, placeholder, width = 320 }: { label: string; value: string | undefined; onChange: (s: string) => void; placeholder?: string; width?: number }) {
  return <Input style={{ ...INPUT, width }} aria-label={label} spellCheck={false} placeholder={placeholder} value={value ?? ''} onChange={(e) => onChange(e.target.value)} />
}

/** Add or edit one output; applied to the page's draft, saved with the settings. */
function OutputEditor({ initial, others, onClose, onApply }: { initial: TakOutput; others: TakOutput[]; onClose: () => void; onApply: (o: TakOutput) => void }) {
  const [o, setO] = useState<TakOutput>(initial)
  const d = o.delivery
  const setD = (patch: object) => setO({ ...o, delivery: { ...d, ...patch } as TakOutput['delivery'] })
  const problem = outputProblem(cleanOutput(o), others)
  const kindName = kindLabel(d.kind)

  return (
    <Modal title={initial.id ? `TAK output: ${initial.id}` : 'Add TAK output'} onClose={onClose} width={640} resizable={false}>
      <form
        className="panel-body stack"
        onSubmit={(e) => {
          e.preventDefault()
          if (!problem) onApply(cleanOutput(o))
        }}
      >
        <Row label="Name" hint="A short name for this output, unique among them: 1 to 32 letters, digits, - or _. It names the output in the logs, the status below and the metrics (sent:<name>).">
          <TextInput label="Name" value={o.id} onChange={(id) => setO({ ...o, id })} width={200} />
        </Row>
        <Row
          label="Delivery"
          hint={
            <>
              TAK Server: OpenTrack connects to a TAK Server&apos;s streaming input, as a TAK client would, and TAK Server passes the tracks on to its users.
              Multicast: UDP datagrams to a group on the local network, which ATAK and WinTAK hear as situational awareness (SA) multicast. Listen for
              clients: OpenTrack is the server; point ATAK or WinTAK at this node (a server connection) and each gets the live picture, then updates.
            </>
          }
        >
          <FieldSelect
            ariaLabel="Delivery"
            fields={TAK_KINDS.map((k) => ({ name: k.label }))}
            value={kindName}
            onChange={(v) => {
              const kind = TAK_KINDS.find((k) => k.label === v)?.kind as TakKind | undefined
              if (kind && kind !== d.kind) setO({ ...o, delivery: newDelivery(kind) })
            }}
            style={{ width: 200 }}
          />
        </Row>
        <Row label="On" hint="Send to this output. Turn it off to stop sending without losing its settings. Changes apply within a few seconds of saving, without a restart.">
          <Toggle value={o.enabled} onChange={(enabled) => setO({ ...o, enabled })} aria-label="On" />
        </Row>
        <Row
          label="Stale after"
          hint="Seconds after a track's last report that TAK treats it as stale and removes it (CoT stale time), 10 to 86400; 60 by default. OpenTrack re-sends a track every half of this until then and then stops, so a track that stops reporting leaves TAK this long after its last report, and comes back when it reports again. Events are timed at the last report, not at sending."
        >
          <div className="num-row">
            <NumberInput label="Stale seconds" value={o.stale_secs} onChange={(stale_secs) => setO({ ...o, stale_secs })} min={10} max={86400} />
            <span className="muted">s</span>
          </div>
        </Row>
        <Row label="Remarks" hint="Put the OpenTrack track number and the sources reporting the track in each event's remarks, which TAK users see in the track's details. A track with a security label always has its marking there, on or off.">
          <Toggle value={o.remarks} onChange={(remarks) => setO({ ...o, remarks })} aria-label="Remarks" />
        </Row>

        {d.kind === 'tak_server' && (
          <>
            <Row label="Host" hint="The TAK Server's host name or IP address.">
              <TextInput label="Host" value={d.host} onChange={(host) => setD({ host })} placeholder="tak.example.mil" width={260} />
            </Row>
            <Row label="Port" hint="TAK Server's streaming input: 8089 for TLS (the usual), 8087 for plain TCP.">
              <NumberInput label="Port" value={d.port} onChange={(port) => setD({ port })} min={1} max={65535} />
            </Row>
            <Row
              label="TLS"
              hint="Encrypt the connection (TAK Server's 8089 input). Off sends the picture in the clear, readable by anyone on the network path: use it only on a trusted network. TLS uses the node's FIPS 140-3 module."
            >
              <Toggle value={d.tls != null} onChange={(on) => setD({ tls: on ? { ca_file: '', cert_file: '', key_file: '' } : undefined })} aria-label="TLS" />
            </Row>
            {d.tls && (
              <>
                <Row label="CA file" hint="PEM file, on this node, of the CA that signed the TAK Server's certificate (TAK Server's truststore CA). Empty: the system's trusted roots. Converting TAK's .p12 files: Help → Administrator guide → TAK certificates.">
                  <TextInput label="CA file" value={d.tls.ca_file} onChange={(ca_file) => setD({ tls: { ...d.tls, ca_file } })} placeholder="/etc/opentrack/tak/ca.pem" />
                </Row>
                <Row
                  label="Client certificate"
                  hint="PEM certificate (chain), on this node, that OpenTrack presents to TAK Server: one TAK Server's CA issued (for example with its makeCert.sh), exported from the .p12 as PEM. TAK Server's 8089 input requires one. Converting TAK's .p12 files: Help → Administrator guide → TAK certificates."
                >
                  <TextInput label="Client certificate" value={d.tls.cert_file} onChange={(cert_file) => setD({ tls: { ...d.tls, cert_file } })} placeholder="/etc/opentrack/tak/opentrack.pem" />
                </Row>
                <Row label="Client key" hint="PEM private key, on this node, of the client certificate (unencrypted; keep the file readable only by OpenTrack). ${env:NAME} references work, as in sources.">
                  <TextInput label="Client key" value={d.tls.key_file} onChange={(key_file) => setD({ tls: { ...d.tls, key_file } })} placeholder="/etc/opentrack/tak/opentrack.key" />
                </Row>
                <Row label="Server name" hint="Check the TAK Server's certificate against this name instead of the host above (when you connect by IP address, say). Empty: the host.">
                  <TextInput label="Server name" value={d.tls.server_name} onChange={(server_name) => setD({ tls: { ...d.tls, server_name } })} width={260} />
                </Row>
              </>
            )}
          </>
        )}

        {d.kind === 'multicast' && (
          <>
            <Row label="Group" hint="The multicast group: 239.2.3.1 for TAK's SA multicast by default, or an IPv6 group such as ff15::6969. A unicast address works too (one receiver, such as a TAK Server's UDP input). Multicast is never encrypted: anyone on the network can read it.">
              <TextInput label="Group" value={d.group} onChange={(group) => setD({ group })} width={160} />
            </Row>
            <Row label="Port" hint="UDP port, 6969 for TAK's SA multicast.">
              <NumberInput label="Port" value={d.port} onChange={(port) => setD({ port })} min={1} max={65535} />
            </Row>
            <Row label="TTL" hint="How many routers the datagrams may cross, 1 to 255 (the hop limit for IPv6); 1 keeps them on this network.">
              <NumberInput label="TTL" value={d.ttl} onChange={(ttl) => setD({ ttl })} min={1} max={255} width={70} />
            </Row>
            <Row label="Interface" hint="This node's network interface to send from, when it has several: its IPv4 address for an IPv4 group, its name (eth0) or index for an IPv6 group. Empty: the system's choice (its default route).">
              <TextInput label="Interface" value={d.interface} onChange={(i) => setD({ interface: i })} placeholder="e.g. 192.168.1.20 or eth0" width={160} />
            </Row>
          </>
        )}

        {d.kind === 'listen' && (
          <>
            <Row label="Listen on" hint="Address and port TAK clients connect to, such as 0.0.0.0:8089 (every interface). In ATAK or WinTAK add a server connection to this node and port, with SSL when TLS is on.">
              <TextInput label="Listen on" value={d.bind} onChange={(bind) => setD({ bind })} width={200} />
            </Row>
            <Row
              label="TLS"
              hint="Encrypt the clients' connections (recommended). Off: plain TCP, where anyone on the network path can read the picture and any client can connect; use it only on a trusted network."
            >
              <Toggle value={d.tls != null} onChange={(on) => setD({ tls: on ? { cert_file: '', key_file: '' } : undefined })} aria-label="TLS" />
            </Row>
            {d.tls && (
              <>
                <Row label="Certificate" hint="PEM server certificate (chain), on this node, that clients check. Clients must trust its CA (in ATAK: the server's truststore). Converting TAK's .p12 files: Help → Administrator guide → TAK certificates.">
                  <TextInput label="Certificate" value={d.tls.cert_file} onChange={(cert_file) => setD({ tls: { ...d.tls, cert_file } })} placeholder="/etc/opentrack/tak/server.pem" />
                </Row>
                <Row label="Key" hint="PEM private key, on this node, of the certificate (keep the file readable only by OpenTrack).">
                  <TextInput label="Key" value={d.tls.key_file} onChange={(key_file) => setD({ tls: { ...d.tls, key_file } })} placeholder="/etc/opentrack/tak/server.key" />
                </Row>
                <Row label="Client CA" hint="PEM CA file: when set, clients must present a certificate it signed (mutual TLS, as TAK Server's 8089 does). Empty: any client that trusts the server may connect. Converting TAK's .p12 files: Help → Administrator guide → TAK certificates.">
                  <TextInput label="Client CA" value={d.tls.client_ca_file} onChange={(client_ca_file) => setD({ tls: { ...d.tls, client_ca_file } })} placeholder="/etc/opentrack/tak/client-ca.pem" />
                </Row>
                {d.tls.client_ca_file?.trim() && (
                  <>
                    <Row label="Certificate optional" hint="Also let clients without a certificate connect (one that presents a certificate must still pass). Off: a certificate is required.">
                      <Toggle value={!!d.tls.client_cert_optional} onChange={(client_cert_optional) => setD({ tls: { ...d.tls, client_cert_optional } })} aria-label="Certificate optional" />
                    </Row>
                    <Row
                      label="Revocation lists"
                      hint="Client certificate revocation lists: PEM or DER files, or directories of them, comma separated. Given, a revoked certificate, one no list covers, or one whose list has expired is refused. Changed lists are picked up within a minute."
                    >
                      <TextInput
                        label="Revocation lists"
                        value={(d.tls.client_crl_files ?? []).join(', ')}
                        onChange={(s) => setD({ tls: { ...d.tls, client_crl_files: s.split(',').map((x) => x.trim()) } })}
                        placeholder="/etc/opentrack/tak/crl/"
                      />
                    </Row>
                  </>
                )}
              </>
            )}
          </>
        )}

        {problem && <span className="muted">{problem}</span>}
        <div className="num-row" style={{ justifyContent: 'flex-end' }}>
          <Button size="sm" variant="ghost" type="button" onClick={onClose}>
            Cancel
          </Button>
          <Button size="sm" type="submit" disabled={!!problem}>
            Apply
          </Button>
        </div>
      </form>
    </Modal>
  )
}

/**
 * Cursor-on-Target outputs to TAK (docs/guides/admin.md#tak-output): a TAK Server, multicast, or clients connecting to this node.
 * The outputs are part of the page's draft; the cot role applies what is saved within a few seconds.
 */
export function TakPanel({
  value,
  onChange,
  dirty,
  saving,
  saved,
  onSave,
}: {
  value: TakSettings | undefined
  onChange: (s: TakSettings) => void
  dirty: boolean
  saving: boolean
  saved: boolean
  onSave: () => void
}) {
  const outputs = value?.outputs ?? []
  const [editing, setEditing] = useState<{ index: number | null; output: TakOutput } | null>(null)
  const [status, setStatus] = useState<TakStatus | null>(null)
  useEffect(() => {
    const load = () => api.takStatus().then(setStatus, () => {})
    load()
    const t = setInterval(load, REFRESH_MS)
    return () => clearInterval(t)
  }, [])
  const statusOf = (id: string): Status | undefined => status?.outputs.find((s) => s.id === id)
  const set = (next: TakOutput[]) => onChange({ outputs: next })

  const columns: DataTableColumn<TakOutput>[] = [
    { key: 'id', header: 'Output', width: 130, mono: true, render: (o) => o.id },
    { key: 'kind', header: 'Delivery', width: 150, render: (o) => kindLabel(o.delivery.kind) },
    { key: 'where', header: 'Address', mono: true, render: (o) => outputWhere(o) },
    {
      key: 'tls',
      header: 'Encryption',
      width: 110,
      render: (o) => (
        <Badge size="sm" color={isEncrypted(o.delivery) ? 'success' : 'warning'}>
          {isEncrypted(o.delivery) ? 'TLS' : 'plaintext'}
        </Badge>
      ),
    },
    {
      key: 'state',
      header: 'Status',
      width: 170,
      render: (o) => {
        const s = statusOf(o.id)
        const state = !o.enabled ? 'off' : s && status?.running ? s.state : 'not started'
        return (
          <span className="num-row">
            <Badge size="sm" color={STATE_COLOR[state] ?? 'grey'}>
              {state}
            </Badge>
            {o.delivery.kind === 'listen' && o.enabled && s && <span className="muted">{s.clients} clients</span>}
          </span>
        )
      },
    },
    { key: 'sent', header: 'Sent', width: 90, mono: true, align: 'right', render: (o) => statusOf(o.id)?.sent ?? 0 },
    {
      key: 'errors',
      header: 'Errors',
      width: 90,
      mono: true,
      align: 'right',
      render: (o) => {
        const s = statusOf(o.id)
        return <span title={s?.last_error ?? undefined}>{s?.errors ?? 0}</span>
      },
    },
    {
      key: 'on',
      header: 'On',
      width: 60,
      render: (o) => (
        <Toggle size="sm" aria-label={`Output ${o.id} on`} value={o.enabled} onChange={(enabled) => set(outputs.map((x) => (x.id === o.id ? { ...x, enabled } : x)))} />
      ),
    },
    {
      key: 'actions',
      header: '',
      width: 90,
      render: (o) => (
        <span className="num-row">
          <Button size="sm" variant="ghost" icon={<TbEdit />} aria-label={`Edit ${o.id}`} onClick={() => setEditing({ index: outputs.indexOf(o), output: o })} />
          <Button size="sm" variant="ghost" icon={<TbTrash />} aria-label={`Delete ${o.id}`} onClick={() => set(outputs.filter((x) => x !== o))} />
        </span>
      ),
    },
  ]
  const errored = (status?.outputs ?? []).filter((s) => s.enabled && s.last_error)

  return (
    <CollapsiblePanel
      title="TAK output"
      persistKey="ot.panel.settings.tak"
      titleActions={
        <InfoTip label="TAK output">
          The published tracks, as Cursor-on-Target events, to TAK: every track OpenTrack publishes to NATS becomes a TAK track (its SIDC or affiliation and
          domain as the symbol, its name or callsign, course, speed and position error), timed at its last report and refreshed until it goes stale, and removed from TAK when it ends. The
          opentrack cot role sends them (opentrack all runs it). Add, change or turn off outputs here, then save: they apply within a few seconds.
        </InfoTip>
      }
      actions={<SaveButton size="sm" dirty={dirty} saving={saving} saved={saved} onSave={onSave} />}
    >
      <div className="panel-body stack">
        {outputs.length > 0 && <DataTable aria-label="TAK outputs" rows={outputs} columns={columns} rowKey={(o) => o.id} density="compact" />}
        <div className="num-row">
          <Button size="sm" variant="ghost" icon={<TbPlus />} onClick={() => setEditing({ index: null, output: newOutput(outputs.map((o) => o.id)) })}>
            Add output
          </Button>
          <span className="muted">
            {status == null ? '' : status.running ? `The cot role is running with ${status.tracks ?? 0} live tracks.` : 'No cot role is running: outputs start when it does.'}
          </span>
          <InfoTip label="Output status">
            Connected (TAK Server), listening (with the clients connected) or sending (multicast) when working; connecting or disconnected while it retries a TAK
            Server (every 1 s, doubling to a minute); error when it cannot start (the reason is on the errors count). Sent: events sent since the role started,
            counting each client. Errors: failed connections, handshakes and writes, and clients dropped for falling behind. Plaintext outputs expose the picture
            to anyone on the network path.
          </InfoTip>
        </div>
        {errored.map((s) => (
          <span key={s.id} className="muted">
            {s.id}: {s.last_error}
          </span>
        ))}
      </div>
      {editing && (
        <OutputEditor
          initial={editing.output}
          others={outputs.filter((_, i) => i !== editing.index)}
          onClose={() => setEditing(null)}
          onApply={(o) => {
            set(editing.index == null ? [...outputs, o] : outputs.map((x, i) => (i === editing.index ? o : x)))
            setEditing(null)
          }}
        />
      )}
    </CollapsiblePanel>
  )
}
