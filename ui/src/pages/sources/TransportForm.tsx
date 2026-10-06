import { useEffect, useState } from 'react'
import { CollapsiblePanel, FieldSelect, Input, Label, Toggle } from '@kineticlogic/staresdk'
import { CodeEditor } from '@kineticlogic/staresdk/code-editor'
import { api, type PluginInfo, type ProtoDescription, type SourceSpec } from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { PluginOptions, PluginPicker } from '../../components/PluginOptions'
import { ProtoSchema } from './ProtoSchema'

type Transport = SourceSpec['transport']
type Codec = SourceSpec['pipeline']['codec']

const TRANSPORTS: Record<string, { label: string; initial: Transport }> = {
  http_poll: { label: 'HTTP poll', initial: { type: 'http_poll', url: '', interval_secs: 5 } },
  websocket: { label: 'WebSocket client', initial: { type: 'websocket', url: '' } },
  tcp_client: { label: 'TCP client', initial: { type: 'tcp_client', host: '', port: 0, framing: { type: 'lines' } } },
  tcp_server: { label: 'TCP server (listen)', initial: { type: 'tcp_server', bind: '0.0.0.0:8087', framing: { type: 'lines' } } },
  udp: { label: 'UDP (unicast or multicast)', initial: { type: 'udp', bind: '0.0.0.0:6969' } },
  mqtt: { label: 'MQTT subscribe', initial: { type: 'mqtt', url: '', topics: [] } },
  grpc_client: { label: 'gRPC client (call the producer)', initial: { type: 'grpc_client', url: '', method: '' } },
  grpc_server: { label: 'gRPC server (producers call)', initial: { type: 'grpc_server', bind: '0.0.0.0:50051' } },
}

const QOS: Record<string, { label: string }> = {
  '0': { label: '0 · at most once' },
  '1': { label: '1 · at least once' },
}

const FRAMINGS: Record<string, { label: string; initial: Record<string, unknown> }> = {
  lines: { label: 'Newline', initial: { type: 'lines' } },
  end_tag: { label: 'End tag (e.g. </event>)', initial: { type: 'end_tag', tag: '</event>' } },
  delimiter: { label: 'Delimiter', initial: { type: 'delimiter', delimiter: '\\n' } },
  length_prefix: { label: 'Length prefix', initial: { type: 'length_prefix', width: 'u32', endian: 'big' } },
  length_field: { label: 'Length field in header', initial: { type: 'length_field', offset: 0, width: 'u32', endian: 'big' } },
}

const ENDIANS: Record<string, { label: string }> = {
  big: { label: 'Big-endian' },
  little: { label: 'Little-endian' },
}

/** Methods offered for an HTTP poll; any other the spec holds is offered too. */
const HTTP_METHODS = ['GET', 'POST', 'PUT', 'PATCH']

const CODECS: Record<string, { label: string; initial: Codec }> = {
  json: { label: 'JSON', initial: { type: 'json' } },
  cot_xml: { label: 'Cursor-on-Target XML', initial: { type: 'cot_xml' } },
  xml: { label: 'XML (generic)', initial: { type: 'xml', record_element: 'record' } },
  protobuf: { label: 'Protobuf (.proto)', initial: { type: 'protobuf', files: {}, message: '' } },
  plugin: { label: 'Plugin', initial: { type: 'plugin', plugin: '', options: {} } },
}

/** A field's label with its ⓘ explanation beside it. */
function FieldLabel({ label, help, htmlFor }: { label: string; help?: React.ReactNode; htmlFor?: string }) {
  return (
    <div className="row-label">
      <Label htmlFor={htmlFor} size="sm">
        {label}
      </Label>
      {help && <InfoTip label={label}>{help}</InfoTip>}
    </div>
  )
}

/** FieldSelect works on names; map labels ↔ ids. */
function Select({
  options,
  value,
  onChange,
  ariaLabel,
}: {
  options: Record<string, { label: string }>
  value: string
  onChange: (id: string) => void
  ariaLabel: string
}) {
  const byLabel = Object.fromEntries(Object.entries(options).map(([id, o]) => [o.label, id]))
  return (
    <FieldSelect
      ariaLabel={ariaLabel}
      fields={Object.values(options).map((o) => ({ name: o.label }))}
      value={options[value]?.label ?? null}
      onChange={(label) => label && onChange(byLabel[label])}
    />
  )
}

function Text({
  label,
  value,
  onChange,
  placeholder,
  wide,
  type = 'text',
  help,
}: {
  label: string
  value: unknown
  onChange: (v: string) => void
  placeholder?: string
  wide?: boolean
  type?: string
  help?: React.ReactNode
}) {
  const id = `f-${label.replace(/\W+/g, '-').toLowerCase()}`
  return (
    <div className={wide ? 'field wide' : 'field'}>
      <FieldLabel htmlFor={id} label={label} help={help} />
      <Input
        id={id}
        type={type}
        value={value === undefined || value === null ? '' : String(value)}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
        spellCheck={false}
        autoComplete="off"
      />
    </div>
  )
}

/** A JSON-valued setting (headers, subscribe message) edited in a small code editor. */
function JsonSetting({ label, value, onChange, help }: { label: string; value: unknown; onChange: (v: unknown) => void; help?: React.ReactNode }) {
  const [text, setText] = useState(() => (value === undefined ? '' : JSON.stringify(value, null, 2)))
  return (
    <div className="field wide">
      <FieldLabel label={label} help={help} />
      <CodeEditor
        aria-label={label}
        value={text}
        minHeight={60}
        maxHeight={180}
        placeholder="JSON, optional. Use ${env:NAME} for secrets."
        onChange={(t) => {
          setText(t)
          if (t.trim() === '') return onChange(undefined)
          try {
            onChange(JSON.parse(t))
          } catch {
            // Keep the last valid value; the editor marks the parse error.
          }
        }}
      />
    </div>
  )
}

/** A free-text setting (an HTTP body) edited in a small code editor. */
function TextSetting({ label, value, onChange, help, placeholder }: { label: string; value: unknown; onChange: (v: string | undefined) => void; help?: React.ReactNode; placeholder?: string }) {
  return (
    <div className="field wide">
      <FieldLabel label={label} help={help} />
      <CodeEditor
        aria-label={label}
        language="text"
        value={typeof value === 'string' ? value : ''}
        minHeight={60}
        maxHeight={180}
        placeholder={placeholder}
        onChange={(t) => onChange(t === '' ? undefined : t)}
      />
    </div>
  )
}

/** A list edited as comma-separated text (e.g. MQTT topic filters). */
function ListSetting({
  label,
  value,
  onChange,
  placeholder,
  help,
}: {
  label: string
  value: unknown
  onChange: (v: string[]) => void
  placeholder?: string
  help?: React.ReactNode
}) {
  const [text, setText] = useState(() => (Array.isArray(value) ? value.join(', ') : ''))
  return (
    <Text
      label={label}
      wide
      value={text}
      placeholder={placeholder}
      help={help}
      onChange={(t) => {
        setText(t)
        onChange(
          t
            .split(',')
            .map((s) => s.trim())
            .filter(Boolean),
        )
      }}
    />
  )
}

const num = (s: string) => (s.trim() === '' ? undefined : Number(s))

/** Transports that can run over TLS (the URL ones only for https://, wss://, mqtts://). */
const TLS_TRANSPORTS = ['tcp_client', 'tcp_server', 'http_poll', 'websocket', 'mqtt', 'grpc_client', 'grpc_server']

/** The transport's `tls` settings: trust, client certificate (mutual TLS) and, for the TCP server, its own certificate. */
function TlsSettings({ transport, onTransport }: { transport: Transport; onTransport: (t: Transport) => void }) {
  const server = transport.type === 'tcp_server' || transport.type === 'grpc_server'
  const tls = (transport.tls as Record<string, unknown> | undefined) ?? {}
  const setTls = (k: string, v: unknown) => {
    const nextTls = { ...tls, [k]: v }
    if (v === undefined || v === '' || v === false || (Array.isArray(v) && v.length === 0)) delete nextTls[k]
    const next: Transport = { ...transport, tls: nextTls }
    // MQTT's older top-level ca_file moves into tls on edit.
    if (k === 'ca_file') delete next.ca_file
    if (Object.keys(nextTls).length === 0) delete next.tls
    onTransport(next)
  }
  const on = transport.tls !== undefined || transport.ca_file !== undefined
  return (
    <div className="field wide">
      <CollapsiblePanel
        title="TLS"
        badge={on ? (tls.cert_file || tls.client_ca_file ? 'mutual' : 'on') : undefined}
        defaultOpen={on}
        titleActions={
          server ? (
            <InfoTip label="TLS">
              Set a certificate and key (PEM files) to accept TLS connections only. With a client CA, every client must present a
              certificate that CA signed (mutual TLS); others are rejected, and each client&apos;s certificate subject is logged. Paths
              may use {'${env:NAME}'}.
            </InfoTip>
          ) : (
            <InfoTip label="TLS">
              For tcp, https://, wss:// and mqtts://. The CA file (PEM) is trusted instead of the system roots. A client certificate
              and key (PEM) authenticate this server to the feed (mutual TLS); set both or neither. Server name verifies the
              feed&apos;s certificate against that name instead of the host. Skipping verification lets anyone impersonate the feed:
              development only. Paths may use {'${env:NAME}'}.
            </InfoTip>
          )
        }
      >
        <div className="panel-body">
          <div className="form-grid">
            {server ? (
              <>
                <Text label="Certificate" value={tls.cert_file} onChange={(v) => setTls('cert_file', v)} placeholder="server.pem" />
                <Text label="Key" value={tls.key_file} onChange={(v) => setTls('key_file', v)} placeholder="server.key" />
                <Text label="Client CA" value={tls.client_ca_file} onChange={(v) => setTls('client_ca_file', v)} placeholder="optional: mutual TLS" />
                <ListSetting
                  label="Client CRLs"
                  value={tls.client_crl_files}
                  onChange={(v) => setTls('client_crl_files', v)}
                  placeholder="optional: /etc/crl/ or files, comma separated"
                  help="Certificate revocation lists (PEM or DER files, or directories of them) for client certificates. Given, a client certificate is refused when revoked, when no list covers its issuer, or when its issuer's list is past its next update, so keep the lists fresh. Needs a client CA."
                />
              </>
            ) : (
              <>
                <Text label="CA file" value={tls.ca_file ?? transport.ca_file} onChange={(v) => setTls('ca_file', v)} placeholder="default: system roots" />
                <Text label="Client certificate" value={tls.cert_file} onChange={(v) => setTls('cert_file', v)} placeholder="optional: mutual TLS" />
                <Text label="Client key" value={tls.key_file} onChange={(v) => setTls('key_file', v)} placeholder="with the certificate" />
                <Text label="Server name" value={tls.server_name} onChange={(v) => setTls('server_name', v)} placeholder="default: the host" />
                <label className="field">
                  <Label size="sm">Skip verification</Label>
                  <Toggle
                    size="sm"
                    value={tls.insecure_skip_verify === true}
                    onChange={(skip) => setTls('insecure_skip_verify', skip)}
                    aria-label="Skip certificate verification"
                  />
                </label>
              </>
            )}
          </div>
        </div>
      </CollapsiblePanel>
    </div>
  )
}

/** Transport and codec settings for a source. */
export function TransportForm({
  transport,
  codec,
  onTransport,
  onCodec,
}: {
  transport: Transport
  codec: Codec
  onTransport: (t: Transport) => void
  onCodec: (c: Codec) => void
}) {
  const set = (k: string, v: unknown) => {
    const next = { ...transport, [k]: v }
    if (v === undefined || v === '') delete next[k]
    onTransport(next)
  }
  const [plugins, setPlugins] = useState<PluginInfo[]>([])
  useEffect(() => {
    if (codec.type !== 'plugin') return
    api.plugins().then(setPlugins, () => setPlugins([]))
  }, [codec.type])
  const plugin = plugins.find((p) => p.name === codec.plugin)
  const pluginOptions = (codec.options as Record<string, unknown> | undefined) ?? {}
  const choosePlugin = (name: string) => {
    const p = plugins.find((x) => x.name === name)
    if (!p) return
    onCodec({ type: 'plugin', plugin: p.name, options: structuredClone(p.default_options) })
    // A stream of the plugin's format is framed its way.
    if (p.framing && (transport.type === 'tcp_client' || transport.type === 'tcp_server')) {
      onTransport({ ...transport, framing: structuredClone(p.framing) })
    }
  }
  // CoT arrives as back-to-back XML events, often over several lines: framed by `</event>`, not by
  // lines. Choosing CoT XML (or a stream transport for it) switches the default lines framing.
  const streamed = (t: Transport) => t.type === 'tcp_client' || t.type === 'tcp_server'
  const cotFraming = (t: Transport, codecType: unknown): Transport =>
    codecType === 'cot_xml' && streamed(t) && (t.framing as { type?: string } | undefined)?.type === 'lines'
      ? { ...t, framing: { type: 'end_tag', tag: '</event>' } }
      : t
  const chooseCodec = (id: string) => {
    onCodec(structuredClone(CODECS[id].initial))
    const next = cotFraming(transport, id)
    if (next !== transport) onTransport(next)
  }
  const [proto, setProto] = useState<ProtoDescription | null>(null)
  const grpc = transport.type === 'grpc_client' || transport.type === 'grpc_server'
  const framing = (transport.framing as Record<string, unknown> | undefined) ?? { type: 'lines' }
  const setFraming = (k: string, v: unknown) => set('framing', { ...framing, [k]: v })

  return (
    <div className="form-grid">
      <div className="field">
        <FieldLabel
          label="Transport"
          help="How frames come off the wire. Client transports connect out to the feed; server, UDP and gRPC server listen for it. A dropped connection or failed run is restarted with backoff. Changing it resets its settings."
        />
        <Select
          ariaLabel="Transport"
          options={TRANSPORTS}
          value={transport.type}
          onChange={(id) => onTransport(cotFraming(structuredClone(TRANSPORTS[id].initial), codec.type))}
        />
      </div>
      <div className="field">
        <FieldLabel
          label="Codec"
          help="How each frame becomes records. JSON: an object, or each element of an array. Cursor-on-Target: one record per <event>. XML: one record per record element. Protobuf: one message per frame, decoded with the producer's .proto files. Plugin: a codec plugin from Settings › Plugins."
        />
        <Select ariaLabel="Codec" options={CODECS} value={codec.type} onChange={chooseCodec} />
      </div>
      {codec.type === 'json' && (
        <Text
          label="Record array path"
          help="Path to the array of records inside each frame, e.g. ac or data.items. Blank: the frame is the record, or each element if the frame is an array. The probe suggests one when frames wrap their records."
          value={codec.records}
          placeholder="blank: auto-detect on probe"
          onChange={(v) => {
            const next: Codec = { ...codec, records: v }
            if (!v) delete next.records
            onCodec(next)
          }}
        />
      )}
      {codec.type === 'plugin' && (
        <>
          <div className="field">
            <FieldLabel
              label="Plugin"
              help="An enabled codec plugin (Settings › Plugins). Choosing one loads its default options and, on TCP, the framing it expects."
            />
            <PluginPicker plugins={plugins} kind="codec" value={codec.plugin as string | undefined} onChange={(p) => choosePlugin(p.name)} width={220} />
          </div>
          <PluginOptions
            plugin={plugin}
            value={pluginOptions}
            onChange={(options) => onCodec({ ...codec, options })}
            row={(key, label, help, control) => (
              <div key={key} className="field">
                <FieldLabel label={label} help={help} />
                {control}
              </div>
            )}
          />
        </>
      )}
      {codec.type === 'protobuf' && <ProtoSchema codec={codec} onCodec={onCodec} onDescription={setProto} />}
      {grpc && codec.type !== 'protobuf' && (
        <div className="field wide notice">gRPC carries protobuf: choose the Protobuf codec and add the producer&apos;s .proto files.</div>
      )}
      {transport.type === 'grpc_client' && (
        <>
          <Text
            label="URL"
            wide
            value={transport.url}
            onChange={(v) => set('url', v)}
            placeholder="https://feed.example:443 (http:// without TLS)"
            help="The producer's gRPC endpoint: http://host:port, or https://host:port for TLS. May use ${env:NAME}."
          />
          <div className="field wide">
            <FieldLabel
              label="Method"
              help="The producer's method to call, from the .proto files (package.Service/Method). Its response type becomes the codec's message: each response is a frame. A stream method keeps sending; a call that ends is made again, with backoff."
            />
            <FieldSelect
              ariaLabel="Method"
              fields={(proto?.methods ?? []).map((m) => ({ name: m.name, type: m.server_streaming ? 'stream' : 'unary' }))}
              value={(transport.method as string) || null}
              onChange={(name) => {
                const m = proto?.methods.find((x) => x.name === name)
                if (!m) return
                onTransport({ ...transport, method: m.name })
                // Frames are the method's responses.
                if (codec.type === 'protobuf' && codec.message !== m.output) {
                  const next: Codec = { ...codec, message: m.output }
                  delete next.records
                  onCodec(next)
                }
              }}
            />
          </div>
          <JsonSetting
            label="Request"
            value={transport.request}
            onChange={(v) => set('request', v)}
            help="The request message as JSON, with field names as in the .proto. Blank: an empty request."
          />
          <JsonSetting
            label="Metadata"
            value={transport.metadata}
            onChange={(v) => set('metadata', v)}
            help={'gRPC metadata sent with the call, as a JSON object of name → value, e.g. {"authorization": "Bearer ${env:TOKEN}"}. Names are lowercase letters, digits, - _ . (not grpc-*, content-type, te or user-agent).'}
          />
          <Text
            label="Keepalive (s)"
            type="number"
            value={transport.keepalive_secs}
            onChange={(v) => set('keepalive_secs', num(v))}
            placeholder="20"
            help="Seconds between HTTP/2 keepalive pings, so a quiet stream through a NAT or firewall is not silently cut. 0: no pings; otherwise 1 to 3600. Default 20."
          />
          <Text
            label="Max message (KiB)"
            type="number"
            value={transport.max_message_kib}
            onChange={(v) => set('max_message_kib', num(v))}
            placeholder="4096"
            help="Largest message accepted, in KiB: 1 to 65536. Default 4096 (4 MiB)."
          />
        </>
      )}
      {transport.type === 'grpc_server' && (
        <>
          <Text
            label="Bind address"
            value={transport.bind}
            onChange={(v) => set('bind', v)}
            placeholder="0.0.0.0:50051"
            help="Address and port to listen on for producers' calls. 0.0.0.0 listens on every interface."
          />
          <ListSetting
            label="Methods producers call"
            help="Methods producers may call, package.Service/Method, comma separated. Every message they send is a frame; a malformed one fails their call with INVALID_ARGUMENT. Blank: every method in the .proto files whose request type is the codec's message."
            value={transport.methods}
            onChange={(v) => (v.length ? onTransport({ ...transport, methods: v }) : set('methods', undefined))}
            placeholder={
              proto?.methods.filter((m) => m.input === codec.message).map((m) => m.name).join(', ') ||
              "blank: every method sending the codec's message"
            }
          />
          <Text
            label="Bearer token"
            value={transport.token}
            onChange={(v) => set('token', v)}
            placeholder="${env:NAME}; blank: none"
            help="If set, producers must send authorization: Bearer <token>; calls without it are refused. Use ${env:NAME} to keep it out of the source's settings. Blank: no token needed."
          />
          <Text
            label="Max connections"
            type="number"
            value={transport.max_connections}
            onChange={(v) => set('max_connections', num(v))}
            placeholder="64"
            help="Producers connected at once, 1 to 10000; more are refused. Default 64."
          />
          <Text
            label="Keepalive (s)"
            type="number"
            value={transport.keepalive_secs}
            onChange={(v) => set('keepalive_secs', num(v))}
            placeholder="20"
            help="Seconds between HTTP/2 keepalive pings to each connected producer. A producer that does not answer a ping within this long (at least 10 s) is disconnected, so a dead or cut-off connection frees its place. 0: no pings; otherwise 1 to 3600. Default 20."
          />
          <Text
            label="Max message (KiB)"
            type="number"
            value={transport.max_message_kib}
            onChange={(v) => set('max_message_kib', num(v))}
            placeholder="4096"
            help="Largest message accepted, in KiB: 1 to 65536. Default 4096 (4 MiB)."
          />
        </>
      )}
      {codec.type === 'xml' && (
        <Text
          label="Record element"
          value={codec.record_element}
          onChange={(v) => onCodec({ ...codec, record_element: v })}
          help="Name of the XML element that is one record, including any namespace prefix as written (e.g. track or ns:track). Each becomes a record keyed by that name; attributes appear as @name, text as #text."
        />
      )}

      {(transport.type === 'http_poll' || transport.type === 'websocket') && (
        <Text
          label="URL"
          wide
          value={transport.url}
          onChange={(v) => set('url', v)}
          placeholder="https://… or wss://… (${env:NAME} for secrets)"
          help={
            transport.type === 'http_poll'
              ? 'The endpoint to poll; each response body is one frame. https:// uses TLS. May use ${env:NAME}.'
              : 'The WebSocket to connect to (ws:// or wss:// for TLS); each message is one frame. May use ${env:NAME}.'
          }
        />
      )}
      {transport.type === 'http_poll' && (
        <>
          <Text
            label="Interval (s)"
            type="number"
            value={transport.interval_secs}
            onChange={(v) => set('interval_secs', num(v))}
            help="Seconds between polls (at least 0.1). Default 5. When the server answers 429 or 503, polling waits for its Retry-After, or backs off (doubling, up to 5 minutes)."
          />
          <Text
            label="Timeout (s)"
            type="number"
            value={transport.timeout_secs}
            onChange={(v) => set('timeout_secs', num(v))}
            placeholder="20"
            help="Seconds a poll may take before it counts as failed. A failed poll is counted as an error and the next one goes ahead. Default 20."
          />
          <JsonSetting
            label="Headers"
            value={transport.headers}
            onChange={(v) => set('headers', v)}
            help={'HTTP headers sent with every poll, as a JSON object of name → value, e.g. {"Authorization": "Bearer ${env:TOKEN}"}.'}
          />
          <div className="field">
            <FieldLabel
              label="Method"
              help="The HTTP method of each poll. Default GET. Use POST (or PUT, PATCH) for an endpoint that takes a query in the request body."
            />
            <Select
              ariaLabel="HTTP method"
              options={Object.fromEntries(
                [...new Set([...HTTP_METHODS, String(transport.method ?? 'GET')])].map((m) => [m, { label: m }]),
              )}
              value={String(transport.method ?? 'GET')}
              onChange={(m) => set('method', m === 'GET' ? undefined : m)}
            />
          </div>
          <TextSetting
            label="Body"
            value={transport.body}
            onChange={(v) => set('body', v)}
            placeholder="optional, e.g. a JSON or GraphQL query"
            help={'Sent exactly as written as the body of every poll. No Content-Type is added: set one in Headers, e.g. {"Content-Type": "application/json"}. May use ${env:NAME}. Blank: no body.'}
          />
        </>
      )}
      {transport.type === 'websocket' && (
        <>
          <Text
            label="In-band error path"
            value={transport.error_path}
            onChange={(v) => set('error_path', v)}
            placeholder="e.g. error"
            help="Path in a JSON message where the server reports errors. If a message has a non-null value there, the connection fails with that error and is reopened. Blank: not checked."
          />
          <Text
            label="Ping every (s)"
            type="number"
            value={transport.ping_secs}
            onChange={(v) => set('ping_secs', num(v))}
            placeholder="20"
            help="Seconds between WebSocket pings that keep the connection alive (at least 1). Default 20."
          />
          <JsonSetting
            label="Subscribe message"
            value={transport.subscribe}
            onChange={(v) => set('subscribe', v)}
            help="Sent once after each connect, e.g. an API key or subscription request. A JSON value is sent as its JSON text; a JSON string is sent as the string itself."
          />
          <JsonSetting
            label="Headers"
            value={transport.headers}
            onChange={(v) => set('headers', v)}
            help={'HTTP headers for the opening handshake, as a JSON object of name → value, e.g. {"Authorization": "Bearer ${env:TOKEN}"}.'}
          />
        </>
      )}
      {transport.type === 'tcp_client' && (
        <>
          <Text
            label="Host"
            value={transport.host}
            onChange={(v) => set('host', v)}
            help="Name or address of the feed's TCP server. A dropped connection is reopened with backoff."
          />
          <Text label="Port" type="number" value={transport.port} onChange={(v) => set('port', num(v))} help="The feed's TCP port." />
          <Text
            label="Send on connect"
            value={transport.send_on_connect}
            onChange={(v) => set('send_on_connect', v)}
            placeholder="optional"
            help="Text sent once after each connect, e.g. a login or subscribe command. Sent exactly as written: no newline is added. May use ${env:NAME}."
          />
        </>
      )}
      {(transport.type === 'tcp_server' || transport.type === 'udp') && (
        <Text
          label="Bind address"
          value={transport.bind}
          onChange={(v) => set('bind', v)}
          placeholder="0.0.0.0:6969"
          help={
            transport.type === 'udp'
              ? 'Address and port to receive datagrams on; each datagram is one frame. 0.0.0.0 listens on every interface.'
              : 'Address and port to listen on. Every client that connects feeds this same source. 0.0.0.0 listens on every interface.'
          }
        />
      )}
      {transport.type === 'udp' && (
        <>
          <Text
            label="Multicast group"
            value={transport.multicast_group}
            onChange={(v) => set('multicast_group', v)}
            placeholder="optional, e.g. 239.2.3.1"
            help="A multicast group to join, IPv4 or IPv6 (bind [::]:port for IPv6). Blank: unicast only."
          />
          <Text
            label="Multicast interface"
            value={transport.multicast_interface}
            onChange={(v) => set('multicast_interface', v)}
            placeholder="optional"
            help="The local interface to join the group on: its IPv4 address for an IPv4 group, its name (eth0) or index for an IPv6 group. Blank: the system's choice."
          />
        </>
      )}
      {transport.type === 'mqtt' && (
        <>
          <Text
            label="Broker URL"
            wide
            value={transport.url}
            onChange={(v) => set('url', v)}
            placeholder="mqtt://host:1883 or mqtts://host:8883"
            help="The MQTT 3.1.1 broker: mqtt://host (port 1883) or mqtts://host (port 8883, TLS). Each message published on a subscribed topic is one frame."
          />
          <ListSetting
            label="Topics"
            value={transport.topics}
            onChange={(v) => onTransport({ ...transport, topics: v })}
            placeholder="comma separated; + and # wildcards, e.g. ais/+/position"
            help="Topic filters to subscribe to; at least one. + matches one level, # the rest. Mappings can read a message's topic as _frame.topic, and its levels as _frame.topic_levels."
          />
          <div className="field">
            <FieldLabel
              label="QoS"
              help="Subscription quality of service. 0: at most once; a message can be lost. 1: at least once; the broker resends until acknowledged, so a message may arrive twice. Default 0."
            />
            <Select ariaLabel="QoS" options={QOS} value={String(transport.qos ?? 0)} onChange={(id) => set('qos', Number(id))} />
          </div>
          <Text
            label="Client id"
            value={transport.client_id}
            onChange={(v) => set('client_id', v)}
            placeholder="default: unique per connection"
            help="The id this connection gives the broker. Blank: a new unique id each connection. Needed for a persistent session."
          />
          <Text label="Username" value={transport.username} onChange={(v) => set('username', v)} placeholder="optional" />
          <Text label="Password" value={transport.password} onChange={(v) => set('password', v)} placeholder="${env:NAME}" />
          <Text
            label="Keepalive (s)"
            type="number"
            value={transport.keepalive_secs}
            onChange={(v) => set('keepalive_secs', num(v))}
            placeholder="30"
            help="Seconds between MQTT keepalive pings, by which the broker notices a dead connection. Default 30."
          />
          <div className="field">
            <FieldLabel
              label="Persistent session"
              help="On: the broker keeps this client's subscriptions, and queues QoS 1 messages while it is disconnected (clean session off). Needs a client id. Off: every connect starts afresh."
            />
            <Toggle
              size="sm"
              value={transport.clean_session === false}
              onChange={(persistent) => set('clean_session', persistent ? false : undefined)}
              aria-label="Persistent session"
            />
          </div>
        </>
      )}
      {TLS_TRANSPORTS.includes(transport.type) && <TlsSettings transport={transport} onTransport={onTransport} />}
      {(transport.type === 'tcp_client' || transport.type === 'tcp_server') && (
        <>
          <div className="field">
            <FieldLabel
              label="Framing"
              help="How the byte stream is cut into frames. Newline: one per line (a trailing \r is dropped, empty lines skipped). End tag: up to and including a closing tag. Delimiter: between separator bytes. Length prefix: a length header, then that many bytes. Length field in header: the header holds the whole frame's length. Frames over 4 MiB are refused."
            />
            <Select
              ariaLabel="Framing"
              options={FRAMINGS}
              value={String(framing.type)}
              onChange={(id) => set('framing', structuredClone(FRAMINGS[id].initial))}
            />
          </div>
          {framing.type === 'end_tag' && (
            <Text
              label="End tag"
              value={framing.tag}
              onChange={(v) => setFraming('tag', v)}
              help="The closing tag that ends each frame, e.g. </event> for Cursor-on-Target. It is kept in the frame; bytes before a frame's first < are discarded."
            />
          )}
          {framing.type === 'delimiter' && (
            <Text
              label="Delimiter"
              value={framing.delimiter}
              onChange={(v) => setFraming('delimiter', v)}
              placeholder="\n, \x03 …"
              help="The bytes between frames; not part of either frame. Escapes: \n, \r, \t, \0 and \xHH (a hex byte). Must not be empty."
            />
          )}
          {framing.type === 'length_field' && (
            <>
              <Text
                label="Length offset (bytes)"
                type="number"
                value={framing.offset}
                onChange={(v) => setFraming('offset', num(v))}
                help="Where the length field sits, in bytes from the start of the frame. The field counts the whole frame, header included (unless adjusted below); the frame keeps its header. STANAG 4607: 2."
              />
              <Text
                label="Length width"
                value={framing.width}
                onChange={(v) => setFraming('width', v)}
                placeholder="u8, u16, u32"
                help="Size of the length field: u8, u16 or u32 (1, 2 or 4 bytes), in the byte order below. STANAG 4607: u32."
              />
              <div className="field">
                <FieldLabel
                  label="Byte order"
                  help="The order of the length field's bytes. Big-endian (network order, most significant byte first) is the default; little-endian puts the least significant first. A u8 field has one byte, so either reads the same. STANAG 4607: big-endian."
                />
                <Select ariaLabel="Length byte order" options={ENDIANS} value={String(framing.endian ?? 'big')} onChange={(e) => setFraming('endian', e)} />
              </div>
              <Text
                label="Length adjustment"
                type="number"
                value={framing.adjust}
                onChange={(v) => setFraming('adjust', num(v))}
                placeholder="0"
                help="Added to the length field's value to get the whole frame's length, for a field that counts only part of the frame: e.g. the length field's offset plus its width when it counts only the bytes after it. May be negative. A result shorter than the header up to the end of the length field is a framing error: the bytes buffered so far are dropped and counted as an error. Default 0: the field counts the whole frame."
              />
            </>
          )}
          {framing.type === 'length_prefix' && (
            <Text
              label="Prefix width"
              value={framing.width}
              onChange={(v) => setFraming('width', v)}
              placeholder="u8, u16, u32, varint"
              help="Size of the length header before each frame: u8, u16 or u32 (1, 2 or 4 bytes, in the byte order below), or varint (protobuf-style). It counts the bytes after it and is not kept in the frame."
            />
          )}
          {framing.type === 'length_prefix' && framing.width !== 'varint' && (
            <div className="field">
              <FieldLabel
                label="Byte order"
                help="The order of the length header's bytes. Big-endian (network order, most significant byte first) is the default; little-endian puts the least significant first (SAPIENT: little-endian u32). A u8 header has one byte, so either reads the same; a varint has its own order."
              />
              <Select ariaLabel="Prefix byte order" options={ENDIANS} value={String(framing.endian ?? 'big')} onChange={(e) => setFraming('endian', e)} />
            </div>
          )}
        </>
      )}
    </div>
  )
}
