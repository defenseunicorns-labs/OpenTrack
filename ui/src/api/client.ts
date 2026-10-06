// Typed client for the OpenTrack control plane (`/api/v1`).

export interface DependencyStatus {
  ok: boolean
  error?: string
  [key: string]: unknown
}

export interface ServerStatus {
  service: string
  version: string
  /** Versions of the correlation engine and trackers (see docs/algorithms.md). */
  algorithms?: { correlation: string; trackers: Record<string, string> }
  site: string
  node_id: string
  sqlite: DependencyStatus & { schema_version?: number; path?: string }
  redis: DependencyStatus & { namespace?: string }
  nats: DependencyStatus & {
    url?: string
    connected?: boolean
    server_name?: string
    server_version?: string
    stream?: string
    tracks_subject?: string
    stream_messages?: number
    stream_bytes?: number
  }
  /** OpenTelemetry export of logs, traces and metrics (not counted in the 503). */
  telemetry?: DependencyStatus & {
    /** Whether any running role exports (an OTEL_EXPORTER_OTLP_*ENDPOINT is set). */
    configured: boolean
    /** "<protocol> <endpoint>" of each exported signal. */
    endpoints?: string[]
  }
}

/** A line of bearing on a track (docs/non-point-contacts.md). */
export interface BearingContact {
  source_id: string
  source_track_key: string
  observed_at: string
  latitude: number
  longitude: number
  bearing_deg: number
  sigma_deg: number
  range_m?: number
  range_sigma_m?: number
  max_range_m?: number
  residual_deg: number
  identifiers?: { scheme: string; value: string }[]
}

/** A bearing or an area instead of a point. */
export type Geometry =
  | {
      type: 'bearing'
      bearing_deg: number
      sigma_deg: number
      range_m?: number
      range_sigma_m?: number
      max_range_m?: number
      elevation_deg?: number
    }
  | { type: 'area'; polygon: [number, number][] }

export interface Observation {
  geometry?: Geometry
  uncertainty?: {
    ellipse?: { semi_major_m: number; semi_minor_m: number; orientation_deg: number }
    circular_error_m?: number
  }
  source_id: string
  source_track_key: string
  name?: string
  callsign?: string
  observed_at: string
  position: { latitude: number; longitude: number; altitude_hae_m?: number }
  kinematics: { course_deg?: number; speed_mps?: number; heading_deg?: number }
  classification: { cot_type?: string; domain?: string; affiliation?: string }
  identifiers?: { scheme: string; value: string }[]
}

/** A track field where the entity's value replaced a different one the feed reported; the entity's is published. */
export interface AttributeNotice {
  /** Track field, e.g. classification.cot_type or ext.destination. */
  key: string
  entity: unknown
  feed: unknown
  source_id: string
}

export interface SystemTrack {
  uid: string
  state: 'tentative' | 'confirmed' | 'lost' | 'dropped'
  view: Observation
  /** With other nodes sharing the picture: the site code of the node that reports it to them. */
  reported_by?: string
  /** Lines of bearing that point at this track, the latest from each sensor. */
  bearings?: BearingContact[]
  /** The registry entity this track resolves to. */
  entity_id?: string
  kind?: 'track' | 'group'
  /** A group's members (UIDs). */
  members?: string[]
  /** Groups it belongs to (`tms-<UID>`). */
  groups?: string[]
  /** Tracks paired with it (UIDs). */
  paired_with?: string[]
  /** Published attributes, resolved from the track's values (feeds and entity links) and built-ins. */
  attributes?: Record<string, unknown>
  notices?: AttributeNotice[]
  contributors: {
    source_id: string
    source_track_key: string
    pairing: 'auto' | 'manual'
    /** Probability that it reports the same object as the rest of the track. */
    confidence: number
    last_report: string
    /** The source's own probability that the object exists (a tracker's). */
    existence?: number
  }[]
  first_seen: string
  last_seen: string
  observation_count: number
  /** False while the track is kept inside OpenTrack (not yet authoritative). */
  published?: boolean
  /** Why the output filter holds it back, when it does. */
  filtered?: string | null
}

export interface TrackResponse {
  /** Probability that the track is a real object (its sources' existence and pairing confidences). */
  confidence?: number
  /** The track's security label as a portion marking, e.g. `(S//REL TO USA, GBR)`; absent when it has none. */
  marking?: string | null
  track: SystemTrack
  /** The `opentrack.track.v2` message as published. */
  message: TrackMessage
  subject: string
}

/** OTH-GOLD minimum plus the output schema's attributes. */
export interface TrackMessage {
  track_id: string
  class: string
  name: string
  domain: string
  affiliation: string
  force_code: number
  track_type: string
  /** Symbol identification code and its standard. */
  sidc: { standard: '2525c' | '2525d' | 'cot'; code: string }
  time: string
  lat: number
  lon: number
  attributes?: Record<string, unknown>
}

export interface GraphEdge {
  id: number
  kind: string
  src_key: string
  dst_key: string
  attrs: Record<string, unknown>
  /** The source node's attributes, e.g. `tracker` for a source track a tracker formed. */
  src_attrs?: Record<string, unknown>
  valid_from_ms: number
  valid_to_ms: number | null
  decision_id: number
  decision_op: string
  decision_actor: string
  decision_reason?: string | null
  ended_by: number | null
}

// --- Correlation ---------------------------------------------------------------------------

/** Correlation settings as the server stores them (see correlate::CorrelationSettings). */
export interface CorrelationSettings {
  approach: 'identifiers' | 'kinematics' | 'kinematics_metadata'
  mode: 'automatic' | 'suggest'
  kinematic: {
    gate_probability: number
    min_sigma_m: number
    process_noise_mps2: number
    speed_sigma_mps: number
    object_density_per_km2: number
    /** Count live tracks within this radius (m) when denser than object_density_per_km2; 0: off. */
    local_density_radius_m: number
    velocity_spread_mps: number
    prior_probability: number
    pair_probability: number
    m: number
    n: number
    window_secs: number
    min_interval_secs: number
    max_age_secs: number
    source_live_secs: number
    stopped_drift_mps: number
    reuse_views: boolean
    reuse_interval_secs: number
    air_velocity_spread_mps: number
  }
  gate: { base_m: number; max_extrapolation_secs: number }
  freshness_secs: number
  split: { propose: boolean; automatic: boolean; split_probability: number; gate_probability: number; m: number; n: number; window_secs: number }
  output: OutputFilter
  /** A scorer plugin whose evidence takes the kinematic comparison's place. */
  scorer?: { plugin: string; options?: Record<string, unknown> } | null
  /** How a fused track's security label is chosen (the highest classification in this order, lowest first). */
  labels?: { classification_order: string[] }
}

export interface CorrelationSettingsResponse {
  settings: CorrelationSettings
  saved: boolean
  defaults: CorrelationSettings
  version: string
}

/** A live track as a suggestion shows it. */
export interface SuggestionTrack {
  uid: string
  track_id: string
  live: boolean
  name?: string | null
  state?: string
  published?: boolean
  latitude?: number
  longitude?: number
  sources?: string[]
}

export interface Suggestion {
  id: number
  kind: 'pair' | 'split'
  track_a: string
  track_b?: string
  source_track?: string
  evidence: Record<string, unknown> & { reason?: string }
  status: 'open' | 'accepted' | 'rejected' | 'expired'
  created_at_ms: number
  updated_at_ms: number
  decision_id?: number
  a: SuggestionTrack
  b: SuggestionTrack | null
}

export interface DecisionRow {
  id: number
  at_ms: number
  actor: string
  op: string
  reason: string | null
  evidence: Record<string, unknown> | null
  /** The decision that undid this one. */
  undone_by?: number
  /** The decision this `undo` undid. */
  undoes?: number
}

/** Track management decisions an undo can reverse (ot_store::undo::UNDOABLE). */
export const UNDOABLE_OPS = [
  'pair_tracks',
  'unpair_tracks',
  'delete_track',
  'merge',
  'split',
  'do_not_pair',
  'create_group',
  'update_group',
  'group_members',
  'dissolve_group',
] as const

/** A system track's position at one time, as published. */
export interface HistoryPoint {
  /** Observed at (Unix ms). */
  t: number
  lat: number
  lon: number
  alt?: number
  course?: number
  speed?: number
}

/** A row of the live tracks list: GOLD fields as published plus what the table shows. */
export interface TrackRow {
  uid: string
  track_id: string
  /** `group` for a group a track manager formed; absent for a track. */
  kind?: 'track' | 'group'
  /** A group's members (`tms-<UID>`). */
  members?: string[]
  /** Groups this track belongs to (`tms-<UID>`). */
  groups?: string[]
  /** Tracks paired with this one (GOLD PAIR), `tms-<UID>`. */
  paired_with?: string[]
  entity_id: string | null
  /** Track fields where the entity replaced what a feed reports. */
  notices: number
  state: SystemTrack['state']
  class: string
  gold_name: string
  /** The track's security label as a portion marking; null when it has none. */
  marking?: string | null
  domain: string
  affiliation: string
  force_code: number
  track_type: string
  sidc: TrackMessage['sidc']
  name?: string | null
  callsign?: string | null
  identifiers?: { scheme: string; value: string }[]
  latitude: number
  longitude: number
  course_deg?: number | null
  speed_mps?: number | null
  last_seen: string
  observation_count: number
  sources: string[]
  published?: boolean
}

export interface Backlog {
  pending: number
  lag: number
}

/** `/metrics`: per-minute counters and gauges, plus live values. */
export interface SystemMetrics {
  minutes: number
  recent_minutes: number
  series: {
    /** Unix minutes. */
    minute: number
    /** Pipeline stage counters summed over sources. */
    ingest: Record<string, number>
    emitted_by_source: Record<string, number>
    engine: Record<string, number>
    writer: Record<string, number>
    /** The TAK output (cot role): sent, errors, dropped, per output as `<counter>:<id>`; gauges clients, tracks. */
    cot?: Record<string, number>
    /** Gauges sampled by the server (last value in the minute). */
    system: Record<string, number>
  }[]
  /** Totals over the last `recent_minutes`. */
  recent: {
    sources: Record<string, Record<string, number>>
    engine: Record<string, number>
    writer: Record<string, number>
  }
  live: {
    tracks: number
    by_state: Record<string, number>
    by_domain: Record<string, number>
    with_entity: number
    notices: number
    outbox: Backlog
    observations: Backlog
    redis_bytes: number
    rss_bytes: number
    threads: number
    nats_messages: number | null
    nats_bytes: number | null
    sqlite_bytes: number
    uptime_secs: number
    cpu_milli: number | null
  }
}

/** `GET /import/config`: whether a configuration may be imported here. */
export interface ConfigImportStatus {
  empty: boolean
  /** What makes the node non-empty, one line each. */
  present: string[]
  format: string
  version: number
}

/** `POST /import/config`: what the import restored. */
export interface ConfigImported {
  imported: boolean
  decision: number
  counts: Record<string, number | boolean>
  notes: string[]
}

export class ApiError extends Error {
  readonly status: number
  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

/** Who the API records as the acting operator until authentication exists. */
const ACTOR = 'op:ui'

/**
 * Every API request says it comes from OpenTrack's own page. The server refuses a change made with
 * the session cookie that lacks it (cross-site request forgery): another site's form or script
 * cannot set it.
 */
export const CSRF_HEADER = 'x-opentrack-csrf'

/** `init` with the CSRF header added. */
export function withCsrf(input: RequestInfo | URL, init: RequestInit | undefined): RequestInit {
  const headers = new Headers(init?.headers ?? (input instanceof Request ? input.headers : undefined))
  headers.set(CSRF_HEADER, '1')
  return { ...init, headers }
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const res = await fetch(`/api/v1${path}`, {
    method,
    headers: {
      'x-opentrack-actor': ACTOR,
      [CSRF_HEADER]: '1',
      ...(body === undefined ? {} : { 'content-type': 'application/json' }),
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const text = await res.text()
  const parsed = text ? JSON.parse(text) : {}
  if (!res.ok) throw new ApiError(res.status, parsed.error ?? res.statusText)
  return parsed as T
}

const get = <T,>(path: string) => request<T>('GET', path)

/** OpenStare's reserved `stare-security` fields; free text until vocabularies are defined. */
export interface SecurityLabel {
  classification: string
  restrictions?: string[]
  sharing?: string
}

/** What OpenTrack publishes (correlation settings `output`). */
export interface OutputFilter {
  areas: { name: string; exclude: boolean; min_lat: number; min_lon: number; max_lat: number; max_lon: number }[]
  affiliations: string[]
  domains: string[]
  track_types: string[]
  min_confidence: number
  rule?: unknown
}

/** The classification banner as OpenStare shapes it. */
export interface Banner {
  enabled: boolean
  text: string
  background: string
  color: string
}

/** The warning users accept after signing in (declining signs them out). */
export interface WarningBanner {
  enabled: boolean
  text: string
}

export interface AppSettings {
  site_name: string
  banner: Banner
  warning: WarningBanner
  /** Hours of position history kept per track (0: none; unset: 12). */
  history_hours?: number | null
  /** At most one history point per track this often, in seconds (0: every update; unset: 10). */
  history_interval_secs?: number | null
  /** Sharing the picture with other OpenTrack nodes. */
  sync?: SyncSettings
  /** XYZ raster tile URL template for the maps' basemap (empty: the country outlines). Admins only: empty for anyone else. */
  basemap_tiles_url?: string
  /** Cursor-on-Target outputs to TAK (the cot role). */
  tak?: TakSettings
}

/** TLS to a TAK Server (as a feed's client TLS). */
export interface TakClientTls {
  ca_file?: string
  cert_file?: string
  key_file?: string
  server_name?: string
}

/** TLS for a listening output (as a feed's server TLS). */
export interface TakServerTls {
  cert_file: string
  key_file: string
  client_ca_file?: string
  client_cert_optional?: boolean
  client_crl_files?: string[]
}

export type TakDelivery =
  | { kind: 'tak_server'; host: string; port: number; tls?: TakClientTls }
  | { kind: 'multicast'; group: string; port: number; ttl: number; interface?: string }
  | { kind: 'listen'; bind: string; tls?: TakServerTls }

/** One TAK output (see docs/guides/admin.md#tak-output). */
export interface TakOutput {
  id: string
  enabled: boolean
  /** Seconds after each send that TAK drops a track; re-sent every half of it. */
  stale_secs: number
  /** Track number and sources in <remarks>. */
  remarks: boolean
  delivery: TakDelivery
}

export interface TakSettings {
  outputs: TakOutput[]
}

/** How each TAK output is going (GET /tak/status). */
export interface TakStatus {
  /** Whether a cot role is writing its status. */
  running: boolean
  at?: string | null
  tracks?: number | null
  outputs: {
    id: string
    kind: TakDelivery['kind']
    enabled: boolean
    encrypted: boolean
    state: string
    clients: number
    sent: number
    errors: number
    dropped: number
    last_error: string | null
  }[]
}

/** What a producer's .proto files define (POST /protobuf/describe). */
export interface ProtoDescription {
  methods: { name: string; input: string; output: string; client_streaming: boolean; server_streaming: boolean }[]
  messages: { name: string; fields: { name: string; type: string; repeated: boolean }[] }[]
}

/** Sharing the picture with other OpenTrack nodes (docs/multi-node.md). */
export interface SyncSettings {
  enabled: boolean
  /** Site codes of the nodes trusted. */
  peers: string[]
  /** Apply other nodes' track management; accept none here. */
  receive_only: boolean
  /** Share the output schema and correlation settings. */
  share_profile: boolean
  /** What this node may send for its tracks, kbit/s (0: no cap). */
  budget_kbps: number
}

export const DEFAULT_SYNC: SyncSettings = { enabled: false, peers: [], receive_only: false, share_profile: true, budget_kbps: 0 }

export interface SyncStatus {
  site: string
  settings: SyncSettings
  log: { heads: { site: string; seq: number }[]; by_status: Record<string, number> }
  /** Written by the link every few seconds; absent when no link runs. */
  link?: {
    at: string
    /** Site code → when last heard. */
    heard: Record<string, string>
    counts: Record<string, number>
  } | null
  engine?: { at: string; reporting: number; shared: number; from_other_nodes: number } | null
}

/** A peer's public key, pinned by an admin: sync messages from its site must verify with it. */
export interface PinnedKey {
  site: string
  public_key: string
  fingerprint: string | null
  pinned_by: string
  pinned_at: string | null
}

/** This node's public key (sync messages it sends are signed with it) and the keys pinned for peers. */
export interface SyncKeys {
  site: string
  public_key: string
  fingerprint: string
  peers: PinnedKey[]
}

// --- Sign-in ---------------------------------------------------------------------------------

export const ROLES = ['viewer', 'track_manager', 'admin'] as const
export type Role = (typeof ROLES)[number]

/** The signed-in account. */
export interface Me {
  id: string
  email: string
  name: string
  role: Role
  via: 'session' | 'api_token' | 'client_cert' | 'openstare' | 'disabled'
  can_change_password: boolean
  /** A temporary or expired password: it must change before anything else. */
  must_change_password?: boolean
  password_expires_at_ms?: number | null
  /** Minutes without use that end this session, in ms. */
  idle_timeout_ms?: number
  password_policy?: PasswordPolicy
  /** The notice-and-consent warning is on and this sign-in hasn't accepted it (the API refuses until it does). */
  consent_required?: boolean
  /** This session's id. */
  session?: string
  /** The previous good sign-in and the failed ones since, as they stood at this sign-in. */
  last_login?: { previous_at_ms: number | null; failed_attempts: number }
  /** False when this address does not serve the admin routes: with OT_ADMIN_BIND they are only on the admin listener. */
  admin_api?: boolean
}

/** What the sign-in page offers. */
export interface AuthPublic {
  /** False when sign-in is turned off (OT_AUTH=off). */
  auth: boolean
  password_login: boolean
  saml: { enabled: boolean; label: string }
  openstare: { enabled: boolean; login_url: string }
}

export interface Account {
  id: string
  email: string
  name: string
  role: Role
  active: boolean
  /** `local`, or `saml` when made at its first single sign-on. */
  origin: string
  has_password: boolean
  created_at_ms: number
  updated_at_ms: number
  last_login_at_ms: number | null
  password_changed_at_ms: number | null
  /** A temporary or expired password, to change at the next sign-in. */
  must_change_password: boolean
  failed_logins: number
  /** Locked until then (9223372036854775807: until an admin unlocks it). */
  locked_until_ms: number | null
  active_since_ms: number | null
  /** Why it was turned off automatically (`inactivity`). */
  disabled_reason: string | null
  /** A temporary account: turned off when this passes (72 hours after it was made). */
  expires_at_ms?: number | null
}

/** A browser session (one sign-in). */
export interface SessionRow {
  id: string
  user_id: string
  user_email: string
  created_at_ms: number
  last_seen_ms: number
  expires_at_ms: number
  ip: string | null
  user_agent: string | null
  ended_at_ms: number | null
  end_reason: string | null
}

export interface ApiTokenRow {
  jti: string
  name: string
  user_id: string
  user_email: string
  created_by: string
  created_at_ms: number
  expires_at_ms: number
  revoked_at_ms: number | null
}

export interface RoleMap {
  value: string
  role: Role
}

export interface AuthSettings {
  disable_password_login: boolean
  saml: {
    enabled: boolean
    idp_metadata_xml: string
    idp_entity_id: string
    sso_url: string
    signing_cert: string
    role_attribute: string
    role_mapping: RoleMap[]
    default_role: Role | null
    allow_admin: boolean
    button_label: string
  }
  openstare: { enabled: boolean; api_url: string; login_url: string; role_mapping: RoleMap[] }
  client_certs: { common_name: string; user: string }[]
  /** Break-glass accounts (emails) inactivity never turns off: Settings → Users → Never turn off. */
  inactivity: { exempt: string[] }
}

/** Rules for local accounts' passwords (fixed at the STIG values; `/auth/me` gives them). */
export interface PasswordPolicy {
  min_length: number
  require_upper: boolean
  require_lower: boolean
  require_digit: boolean
  require_special: boolean
  history: number
  min_changed_chars: number
  min_age_hours: number
  max_age_days: number
  /** Commonly used passwords, and ones built from them, are refused. */
  refuse_common?: boolean
}

/** The password rules in words, for a password field's ⓘ. */
export function describePolicy(p: PasswordPolicy | undefined): string {
  if (!p) return 'At least 15 characters, with an upper-case letter, a lower-case letter, a digit and a special character.'
  const classes = [
    p.require_upper && 'an upper-case letter',
    p.require_lower && 'a lower-case letter',
    p.require_digit && 'a digit',
    p.require_special && 'a special character',
  ].filter(Boolean)
  return (
    `At least ${p.min_length} characters` +
    (classes.length ? `, with ${classes.join(', ')}` : '') +
    (p.history > 0 ? `; not one of the last ${p.history}` : '') +
    (p.refuse_common ? '; not a commonly used password or one built from it' : '') +
    '.'
  )
}

/** Auth settings as read: with what this build and deployment fix. */
export interface AuthSettingsResponse extends AuthSettings {
  build: { saml: boolean; public_url: string | null }
}

export interface AppSettingsResponse {
  settings: AppSettings
  site_code: string
  node_id: string
  /** Whether the maps show basemap tiles (whoever asks; the URL is an admin's). */
  basemap_tiles: boolean
}

/** What a spreadsheet import does, row by row. */
export interface SheetImport {
  applied: boolean
  counts: { create: number; update: number; unchanged: number; error: number }
  rows: {
    row: number
    action: 'create' | 'update' | 'unchanged' | 'error'
    entity_id: string
    name?: string
    identifiers_added?: string[]
    /** Fields the row changes (the minimum's and attributes). */
    fields?: string[]
    errors?: string[]
  }[]
}

/** A codec plugin and the options it takes. */
/** One option a plugin takes, as its manifest describes it. */
export type PluginOption =
  | { name: string; label: string; help: string; type: 'bool'; default: boolean }
  | { name: string; label: string; help: string; type: 'choice'; choices: string[]; default: string }
  | { name: string; label: string; help: string; type: 'number'; default: number | null; unit: string; min: number | null }
  | { name: string; label: string; help: string; type: 'text'; default: string | null }

export type PluginKind = 'codec' | 'tracker' | 'scorer'

/** Plugin grants: what it may use beyond computing (WebAssembly plugins). */
export interface PluginGrants {
  memory_mb: number
  call_timeout_ms: number
  dirs: { path: string; guest?: string; write?: boolean }[]
  network: string[]
  env: Record<string, string>
}

/** A plugin: built in, a WebAssembly component, or an external program. */
export interface PluginInfo {
  name: string
  version: string
  description: string
  kinds: PluginKind[]
  options: PluginOption[]
  default_options: Record<string, unknown>
  framing: Record<string, unknown> | null
  builtin: boolean
  runtime: 'builtin' | 'wasm' | 'external'
  enabled: boolean
  status: 'loaded' | 'loading' | 'error' | 'disabled'
  error?: string
  sha256?: string | null
  size?: number | null
  address?: string | null
  /** An external plugin: whether it has a secret (the secret is never returned). */
  secret_set?: boolean
  grants?: PluginGrants
  updated_at_ms?: number
  used_by: { source?: string; name?: string; as: PluginKind; enabled?: boolean; correlation?: boolean }[]
}

/** What `check` found: whether the plugin loads and opens each kind. */
export interface PluginCheck {
  ok: boolean
  error?: string
  load_ms?: number
  kinds?: Record<string, 'ok' | { error: string }>
}

/** A sensor's tracker settings, as a file (profiles/trackers). */
export interface TrackerProfile {
  name: string
  label: string
  description: string
  sensor: { kind?: string; platform?: string; band?: string; domain?: string; [k: string]: unknown }
  basis: string
  tracker: Record<string, unknown>
  builtin?: boolean
}

/** @deprecated the codec plugins are PluginInfo with kind `codec`. */
export type CodecPlugin = PluginInfo
const enc = encodeURIComponent

// --- Sources -------------------------------------------------------------------------------

/** A source spec as stored: transport + pipeline, all JSON. */
export interface SourceSpec {
  id: string
  name: string
  description?: string
  transport: { type: string; [k: string]: unknown }
  pipeline: {
    codec: { type: string; [k: string]: unknown }
    mapping: { schema_version?: number; rules: unknown[]; reject?: unknown[] }
    [k: string]: unknown
  }
  priority?: number
  /** Tracks (a key per object) or detections (anonymous plots). */
  reports?: 'tracks' | 'detections'
  /** Whether a track this source alone reports for is published (default: track feeds only). */
  publish_alone?: boolean
  /** Reports a new track needs from this source to be confirmed; unset: 1 for track feeds, the engine's default for detections. */
  confirm_after?: number
  /** Security label for everything the source reports (OpenStare's `stare-security` shape). */
  security?: SecurityLabel
  /** A listener that does not authenticate its senders runs only with this risk acceptance recorded (UDP always needs it). */
  unauthenticated?: 'accepted'
  /** For a source reporting lines of bearing: how the emitters it hears may move (unset: 0.1 m/s², 30 m/s). */
  emitter_motion?: EmitterMotion
}

/** How the emitters a bearing source hears may move; bounds the error of locating them from one moving sensor. */
export interface EmitterMotion {
  /** The hardest an emitter may manoeuvre unseen by a constant-velocity fit (m/s²). */
  manoeuvre_mps2: number
  /** The fastest an emitter may move (m/s). */
  max_speed_mps: number
}

export interface LinkStatus {
  connected: boolean
  connects: number
  errors: number
  last_error?: string
  last_frame_at?: string
}

export interface SourceStatus {
  source: string
  revision: number
  transport: string
  link: LinkStatus
  last_error: string | null
  /** Set when the worker refuses to start the source, saying why (e.g. a listener without sender authentication). */
  not_started?: string
  totals_since_start: Record<string, number>
  /** The windows a tracker with auto timing chose from the sensor's revisit rate. */
  tracker_timing?: {
    revisit_secs: number
    revisit_source: string
    confirm_within_secs: number
    drop_tentative_secs: number
    drop_confirmed_secs: number
  } | null
  updated_at: string
}

export interface SourceRow {
  id: string
  name: string
  transport: string
  codec: string
  enabled: boolean
  priority: number
  revision: number
  spec: SourceSpec
  raw_subject: string | null
  created_at_ms: number
  updated_at_ms: number
  /** Live worker status; null when no worker runs it (disabled or stopped). */
  status: SourceStatus | null
}

export interface MetricsResponse {
  source: string
  minutes: number
  totals: Record<string, number>
  series: { minute: number; counts: Record<string, number> }[]
}

// --- Probe and preview ---------------------------------------------------------------------

export interface FieldStat {
  path: string
  mapping_path: string
  types: Record<string, number>
  present: number
  presence: number
  distinct: number
  distinct_capped: boolean
  samples: unknown[]
  min?: number
  max?: number
}

export interface Proposal {
  target: string
  spec: unknown
  confidence: number
  reason: string
}

export interface ProbeResult {
  frames: number
  seconds: number
  link_error: string | null
  /** Bytes a stream transport read, framed or not. */
  bytes_received?: number
  /** Why no frames arrived, when no error says (connected but silent, or unframed bytes). */
  hint?: string | null
  decode_errors: number
  last_decode_error: string | null
  records: number
  codec: SourceSpec['pipeline']['codec']
  suggested_records_path: string | null
  fields: FieldStat[]
  suggestion: { mapping: SourceSpec['pipeline']['mapping']; proposals: Proposal[]; identifiers: unknown[]; missing: string[] }
  samples_saved: number | null
  sample_frames: {
    received_at: string
    origin: string | null
    bytes: number
    /** Transport metadata, e.g. the MQTT `topic`; also under `_frame` in records. */
    meta?: Record<string, unknown> | null
    text: string
    truncated: boolean
  }[]
}

export interface PreviewObservation extends Observation {
  identifiers?: { scheme: string; value: string }[]
  ext?: Record<string, unknown>
  provenance?: { confidence?: number; source_code?: string }
  platform?: Record<string, string>
}

export interface PreviewResult {
  valid: boolean
  counts?: Record<string, number>
  errors?: string[]
  observations?: PreviewObservation[]
  /** The first decoded records stage by stage, when the preview asked for a trace. */
  trace?: PreviewTrace
}

/** A dry run's first records (samples), each followed through the pipeline. */
export interface PreviewTrace {
  /** The frames the samples came from, as received, each once. */
  frames: TraceFrameView[]
  samples: TraceSample[]
}

/** A frame as received: parsed JSON, else its text, else its bytes escaped; cut at 64 KiB. */
export interface TraceFrameView {
  format: 'json' | 'text' | 'binary'
  bytes: number
  /** The parsed JSON, or text; a JSON frame too big to send whole is its pretty-printed start (a string). */
  content: unknown
  /** Only the start of the frame is in `content`. */
  truncated?: boolean
}

/** One record (or a frame that did not decode), and what each stage made of it. */
export interface TraceSample {
  /** Index in `frames` of the frame it came from. */
  frame: number
  /** The pipeline's stages after the transport, in order (ids as in `pipelineStages`), ending with `publish`. */
  stages: TraceStage[]
}

export interface TraceStage {
  id: string
  /** What the stage passed on: the record up to Reject, the mapping's outputs, then observations, the published message at Publish. */
  items: unknown[]
  /** Why it (or one of its outputs) was dropped at this stage. */
  dropped?: string[]
  /** What the stage did with it that is not a drop (a static identity kept, say): shown at this stage only. */
  notes?: string[]
  /** The stage held it back (a tracker waiting for the rest of a scan). */
  held?: boolean
}

// --- Schema --------------------------------------------------------------------------------

export interface ExtensionField {
  key: string
  type: 'string' | 'integer' | 'number' | 'boolean' | 'enum' | 'timestamp' | 'position' | 'json'
  unit?: string
  required?: boolean
  default?: unknown
  enum_values?: string[]
  /** Notes for the people who maintain and consume the schema. */
  description?: string
  /** Filled by OpenTrack (state, speed_mps, ...) instead of a feed or entity. */
  builtin?: string
}

export interface SchemaVersion {
  version: number
  status: 'draft' | 'published'
  published_at_ms: number | null
  notes: string | null
  fields: ExtensionField[]
}

export interface SchemaOverview {
  /** Always published: the OTH-GOLD minimum. */
  published_core: string[]
  /** OpenTrack values a field can be linked to, with the type the field must have. */
  builtins: { name: string; type: ExtensionField['type']; reads?: string[] }[]
  /** Which observation fields (mapping targets) fill each always-published field. */
  gold_sources?: Record<string, string[]>
  core: string[]
  reserved_extension_keys: string[]
  latest_published: number | null
  versions: SchemaVersion[]
  sources: { source: string; enabled: boolean; schema_version: number }[]
}

// --- Track management ----------------------------------------------------------------------

/** What a group is: name and symbol, plus the symbol's parts the editor keeps. */
export interface GroupSpec {
  name: string
  sidc: string
  affiliation?: string | null
  domain?: string | null
  description?: string | null
  /** Group class published as the GOLD class-name, e.g. CARRIER STRIKE GROUP. */
  class?: string | null
  /** How the symbol was built (the editor's choices). */
  base?: string | null
  echelon?: string | null
  task_force?: boolean
  sidc_override?: boolean
}

export interface TrackGroup {
  track_id: string
  uid: string
  spec: GroupSpec
  members: string[]
  created_at_ms: number
  updated_at_ms: number
}

// --- Entities --------------------------------------------------------------------------

export interface RegistryIdentifier {
  scheme: string
  value: string
  /** Name the object is expected to broadcast under this identifier. */
  expected_name?: string | null
  source?: string | null
}

export type AttrType = 'text' | 'number' | 'boolean' | 'datetime' | 'json'
export const ATTR_TYPES: AttrType[] = ['text', 'number', 'boolean', 'datetime', 'json']

export interface EntityAttribute {
  key: string
  type: AttrType
  value: unknown
}

export const DOMAINS = ['air', 'surface', 'subsurface', 'ground', 'space'] as const
export const AFFILIATIONS = ['pending', 'unknown', 'assumed_friend', 'friend', 'neutral', 'suspect', 'hostile', 'joker', 'faker', 'none'] as const
export const TRACK_TYPES = ['tactical', 'live_training', 'simulated_training', 'demand_entry'] as const

/** One real-world object: identifiers, status, the OTH-GOLD minimum and free-form attributes. */
export interface Entity {
  id: string
  status: 'active' | 'retired'
  name?: string | null
  class_name?: string | null
  domain?: (typeof DOMAINS)[number] | null
  affiliation?: (typeof AFFILIATIONS)[number] | null
  track_type?: (typeof TRACK_TYPES)[number] | null
  cot_type?: string | null
  sidc?: string | null
  /** A track manager's override of publishing its tracks; unset: the engine's rules. */
  publish?: 'always' | 'never' | null
  identifiers: RegistryIdentifier[]
  attributes: EntityAttribute[]
  source?: string | null
  updated_at_ms?: number
}

export interface EntityView {
  entity: Entity
  /** Saved versions, newest first. */
  revisions: { id: number; entity: Entity; saved_at_ms: number; decision_id: number | null; actor: string | null }[]
  /** Live tracks resolving to it, with the fields where it replaced what their feed reports. */
  tracks: { uid: string; track_id: string; state: string; source_id: string; last_seen: string; notices: AttributeNotice[] }[]
}

export const api = {
  // 503 while SQLite, Redis or NATS is down: the body still says which.
  status: async (): Promise<ServerStatus> => {
    const res = await fetch('/api/v1/status', { headers: { 'x-opentrack-actor': ACTOR } })
    const text = await res.text()
    const parsed = text ? JSON.parse(text) : {}
    if (!res.ok && !(res.status === 503 && parsed.service)) throw new ApiError(res.status, parsed.error ?? res.statusText)
    return parsed as ServerStatus
  },
  syncStatus: () => get<SyncStatus>('/sync/status'),
  syncKeys: () => get<SyncKeys>('/sync/keys'),
  pinSyncKey: (site: string, publicKey: string) => request<SyncKeys>('PUT', `/sync/keys/${enc(site)}`, { public_key: publicKey }),
  removeSyncKey: (site: string) => request<SyncKeys>('DELETE', `/sync/keys/${enc(site)}`),
  takStatus: () => get<TakStatus>('/tak/status'),
  describeProtobuf: (files: Record<string, string>) => request<ProtoDescription>('POST', '/protobuf/describe', { files }),
  systemMetrics: (minutes = 60) => get<SystemMetrics>(`/metrics?minutes=${minutes}`),
  tracks: (limit = 10000) => get<{ total: number; tracks: TrackRow[] }>(`/tracks?limit=${limit}`),
  track: (uid: string) => get<TrackResponse>(`/tracks/${enc(uid)}`),
  explain: (uid: string) => get<{ uid: string; edges: GraphEdge[] }>(`/tracks/${enc(uid)}/explain`),
  correlationSettings: () => get<CorrelationSettingsResponse>('/correlation/settings'),
  saveCorrelationSettings: (s: CorrelationSettings) => request<{ settings: CorrelationSettings }>('PUT', '/correlation/settings', s),
  suggestionCount: () => get<{ open: number }>('/correlation/suggestions/count'),
  suggestions: (status = 'open') => get<{ suggestions: Suggestion[] }>(`/correlation/suggestions?status=${enc(status)}`),
  decideSuggestion: (id: number, decision: 'accept' | 'reject') =>
    request<Record<string, unknown>>('POST', `/correlation/suggestions/${id}/${decision}`),
  decisions: (ops: readonly string[], limit = 200) => get<{ decisions: DecisionRow[] }>(`/decisions?op=${enc(ops.join(','))}&limit=${limit}`).then((r) => r.decisions),
  undoDecision: (id: number, reason?: string) => request<Record<string, unknown>>('POST', `/decisions/${id}/undo`, reason ? { reason } : {}),
  trackHistory: (uid: string, limit = 5000) => get<{ track_id: string; points: HistoryPoint[] }>(`/tracks/${enc(uid)}/history?limit=${limit}`),
  deleteHistoryPoint: (uid: string, t: number, reason?: string) =>
    request<{ decision: number; deleted: HistoryPoint; stepped_back: boolean }>('POST', `/history/${enc(uid)}/delete`, { t, ...(reason ? { reason } : {}) }),
  correlationDecisions: (limit = 100) => get<{ decisions: DecisionRow[] }>(`/correlation/decisions?limit=${limit}`),
  splitTrack: (uid: string, sourceTrack: string) =>
    request<{ new_track: string }>('POST', `/tracks/${enc(uid)}/split`, { source_track: sourceTrack }),
  /** Merge `from` into `into`; with `hold` (a track manager's merge, GOLD MRG) correlation never splits it. */
  mergeTracks: (from: string, into: string, hold = false) => request<{ merged_into: string }>('POST', '/tracks/merge', { from, into, hold }),
  pairTracks: (tracks: string[]) => request<{ paired: string[] }>('POST', '/tracks/pair', { tracks }),
  unpairTracks: (a: string, b: string) => request<unknown>('POST', '/tracks/unpair', { a, b }),
  deleteTracks: (tracks: string[]) => request<{ deleted: string[] }>('POST', '/tracks/delete', { tracks }),
  groups: () => get<{ groups: TrackGroup[] }>('/groups').then((r) => r.groups),
  createGroup: (spec: GroupSpec, members: string[]) => request<{ group: string }>('POST', '/groups', { spec, members }),
  updateGroup: (id: string, spec: GroupSpec) => request<unknown>('PUT', `/groups/${enc(id)}`, { spec }),
  groupMembers: (id: string, add: string[], remove: string[]) => request<unknown>('POST', `/groups/${enc(id)}/members`, { add, remove }),
  dissolveGroup: (id: string) => request<unknown>('DELETE', `/groups/${enc(id)}`),
  doNotPair: (a: string, b: string) => request<Record<string, unknown>>('POST', '/tracks/do-not-pair', { a, b }),

  sources: () => get<{ sources: SourceRow[] }>('/sources').then((r) => r.sources),
  source: (id: string) => get<SourceRow>(`/sources/${enc(id)}`),
  createSource: (spec: SourceSpec) => request<SourceRow>('POST', '/sources', spec),
  saveSource: (spec: SourceSpec) => request<SourceRow>('PUT', `/sources/${enc(spec.id)}`, spec),
  deleteSource: (id: string) => request<unknown>('DELETE', `/sources/${enc(id)}`),
  setEnabled: (id: string, enabled: boolean) =>
    request<SourceRow>('POST', `/sources/${enc(id)}/${enabled ? 'enable' : 'disable'}`),
  metrics: (id: string, minutes = 60) => get<MetricsResponse>(`/sources/${enc(id)}/metrics?minutes=${minutes}`),
  revisions: (id: string) =>
    get<{ revisions: { revision: number; saved_at_ms: number; decision_id: number | null; spec: SourceSpec }[] }>(
      `/sources/${enc(id)}/revisions`,
    ).then((r) => r.revisions),

  plugins: () => get<{ plugins: PluginInfo[] }>('/plugins').then((r) => r.plugins),
  trackerProfiles: () => get<{ profiles: TrackerProfile[]; problems: string[] }>('/tracker-profiles'),
  saveTrackerProfile: (p: TrackerProfile, replace = false) =>
    request<TrackerProfile>('POST', `/tracker-profiles${replace ? '?replace=true' : ''}`, p),
  deleteTrackerProfile: (name: string) => request<unknown>('DELETE', `/tracker-profiles/${enc(name)}`),
  /** Add a WebAssembly component (or, with replace, a new build of one). */
  addPluginWasm: async (file: File, replace = false): Promise<PluginInfo> => {
    const res = await fetch(`/api/v1/plugins${replace ? '?replace=true' : ''}`, {
      method: 'POST',
      headers: { 'x-opentrack-actor': ACTOR, [CSRF_HEADER]: '1', 'content-type': 'application/wasm' },
      body: file,
    })
    const text = await res.text()
    const parsed = text ? JSON.parse(text) : {}
    if (!res.ok) throw new ApiError(res.status, parsed.error ?? res.statusText)
    return parsed as PluginInfo
  },
  addPluginExternal: (address: string, secret: string, replace = false) =>
    request<PluginInfo>('POST', `/plugins${replace ? '?replace=true' : ''}`, { address, ...(secret ? { secret } : {}) }),
  /** A new external-plugin secret from the server's FIPS DRBG; not stored, shown once. */
  newPluginSecret: () => request<{ secret: string }>('POST', '/plugins/secret'),
  configurePlugin: (name: string, body: { enabled?: boolean; grants?: PluginGrants; secret?: string }) =>
    request<PluginInfo>('PUT', `/plugins/${enc(name)}`, body),
  deletePlugin: (name: string, force = false) => request<unknown>('DELETE', `/plugins/${enc(name)}${force ? '?force=true' : ''}`),
  checkPlugin: (name: string) => request<PluginCheck>('POST', `/plugins/${enc(name)}/check`),

  probe: (body: {
    transport: SourceSpec['transport']
    codec: SourceSpec['pipeline']['codec']
    max_frames?: number
    max_secs?: number
    save_as?: string
  }) => request<ProbeResult>('POST', '/probe', body),
  /** A dry run; `trace` > 0 also follows that many decoded records stage by stage (at most 10). */
  preview: (spec: SourceSpec, storedSamplesOf?: string, samples: string[] = [], trace = 0) =>
    request<PreviewResult>('POST', '/sources/validate', { spec, samples, stored_samples_of: storedSamplesOf, ...(trace > 0 ? { trace } : {}) }),

  schema: () => get<SchemaOverview>('/schema'),
  saveDraft: (fields: ExtensionField[], notes?: string) =>
    request<SchemaVersion>('PUT', '/schema/draft', { fields, notes }),
  publishDraft: () => request<SchemaVersion>('POST', '/schema/draft/publish'),
  discardDraft: () => request<unknown>('DELETE', '/schema/draft'),

  appSettings: () => get<AppSettingsResponse>('/settings'),
  saveAppSettings: (settings: AppSettings) => request<AppSettingsResponse>('PUT', '/settings', settings),
  banner: () => get<Banner>('/public/banner'),
  warningBanner: () => get<WarningBanner>('/public/warning-banner'),

  authPublic: () => get<AuthPublic>('/auth/public'),
  me: () => get<Me>('/auth/me'),
  login: (email: string, password: string) => request<Me>('POST', '/auth/login', { email, password }),
  logout: () => request<unknown>('POST', '/auth/logout'),
  /** Accept the notice-and-consent warning, for this sign-in (recorded and audited on the server). */
  acceptConsent: () => request<{ consent_required: boolean }>('POST', '/auth/consent'),
  changePassword: (current: string, next: string) => request<Me>('POST', '/auth/password', { current, new: next }),
  users: () => get<{ users: Account[] }>('/auth/users').then((r) => r.users),
  createUser: (u: { email: string; name: string; role: Role; password?: string; temporary?: boolean }) => request<Account>('POST', '/auth/users', u),
  updateUser: (id: string, change: { name?: string; role?: Role; active?: boolean; temporary?: boolean }) => request<Account>('PUT', `/auth/users/${enc(id)}`, change),
  deleteUser: (id: string) => request<unknown>('DELETE', `/auth/users/${enc(id)}`),
  /** Set a new password, or with `null` remove it (single sign-on only). */
  resetPassword: (id: string, password: string | null) => request<unknown>('POST', `/auth/users/${enc(id)}/password`, { password }),
  revokeSessions: (id: string) => request<unknown>('POST', `/auth/users/${enc(id)}/revoke`),
  unlockUser: (id: string) => request<Account>('POST', `/auth/users/${enc(id)}/unlock`),
  sessions: (q: { all?: boolean; user?: string } = {}) =>
    get<{ sessions: SessionRow[]; current: string | null }>(
      `/auth/sessions${q.all ? '?all=true' : q.user ? `?user=${enc(q.user)}` : ''}`,
    ),
  endSession: (id: string) => request<unknown>('DELETE', `/auth/sessions/${enc(id)}`),
  apiTokens: () => get<{ tokens: ApiTokenRow[] }>('/auth/api-tokens').then((r) => r.tokens),
  createApiToken: (t: { name: string; user_id?: string; days?: number }) =>
    request<{ token: string; jti: string; name: string; user: string; expires_at_ms: number }>('POST', '/auth/api-tokens', t),
  revokeApiToken: (jti: string) => request<unknown>('DELETE', `/auth/api-tokens/${enc(jti)}`),
  authSettings: () => get<AuthSettingsResponse>('/auth/settings'),
  saveAuthSettings: (s: AuthSettings) => request<AuthSettingsResponse>('PUT', '/auth/settings', s),
  parseSamlMetadata: (xml: string) =>
    request<{ idp_metadata_xml: string; idp_entity_id: string; sso_url: string; signing_cert: string }>('POST', '/auth/saml/parse-metadata', { xml }),
  exportUrl: (what: 'tracks.geojson' | 'tracks.csv' | 'config') =>
    what === 'config' ? '/api/v1/export/config' : `/api/v1/export/tracks?format=${what === 'tracks.csv' ? 'csv' : 'geojson'}`,
  /** Whether this node may import a configuration (it has none yet), and if not, what it has. Admins. */
  configImportStatus: () => get<ConfigImportStatus>('/import/config'),
  /** Rebuild this empty node from a full configuration export. The caller's account is replaced. */
  importConfig: async (file: File): Promise<ConfigImported> => {
    const res = await fetch('/api/v1/import/config', {
      method: 'POST',
      headers: { 'x-opentrack-actor': ACTOR, [CSRF_HEADER]: '1', 'content-type': 'application/json' },
      body: file,
    })
    const text = await res.text()
    let parsed: { error?: string; problems?: string[] } & Partial<ConfigImported> = {}
    try {
      parsed = text ? JSON.parse(text) : {}
    } catch {
      parsed = { error: text }
    }
    if (!res.ok) {
      const detail = parsed.problems?.length ? `${parsed.problems.join('; ')}` : parsed.error
      throw new ApiError(res.status, detail ?? res.statusText)
    }
    return parsed as ConfigImported
  },
  purge: (confirm: string, history: boolean) =>
    request<{ retired: number; history?: { nodes: number; edges: number } | null }>('POST', '/admin/purge', { confirm, history }),

  registryEntities: (q: string, limit = 100, offset = 0) =>
    get<{ entities: Entity[]; total: number }>(`/registry/entities?q=${enc(q)}&limit=${limit}&offset=${offset}`),
  entity: (id: string) => get<EntityView>(`/registry/entities/${enc(id)}`),
  /** Create an entity; with `from_track`, seeded with that live track's name and identifiers. */
  createEntity: (entity: Omit<Entity, 'id'> & { from_track?: string }) => request<EntityView>('POST', '/registry/entities', entity),
  saveEntity: (entity: Entity) => request<EntityView>('PUT', `/registry/entities/${enc(entity.id)}`, entity),
  deleteEntity: (id: string) => request<unknown>('DELETE', `/registry/entities/${enc(id)}`),
  /** Entity fields a pipeline can link: the minimum and the attribute keys in use. */
  registryFields: () => get<{ minimum: string[]; attributes: { key: string; entities: number }[] }>('/registry/fields'),
  registryExportUrl: (format: 'xlsx' | 'csv') => `/api/v1/registry/export?format=${format}`,
  /** Plan (or with `apply`, make) the changes a spreadsheet describes. */
  importSheet: async (file: File, apply: boolean): Promise<SheetImport> => {
    const format = file.name.toLowerCase().endsWith('.csv') ? 'csv' : 'xlsx'
    const res = await fetch(`/api/v1/registry/import-sheet?format=${format}&apply=${apply}&label=${enc(file.name)}`, {
      method: 'POST',
      headers: { 'x-opentrack-actor': ACTOR, [CSRF_HEADER]: '1' },
      body: file,
    })
    const parsed = JSON.parse((await res.text()) || '{}')
    if (!res.ok && !parsed.rows) throw new ApiError(res.status, parsed.error ?? res.statusText)
    return parsed as SheetImport
  },

}
