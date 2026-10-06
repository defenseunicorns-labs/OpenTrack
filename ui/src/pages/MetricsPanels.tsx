import { useEffect, useMemo, useState } from 'react'
import 'uplot/dist/uPlot.min.css'
import { CollapsiblePanel, useToast } from '@kineticlogic/staresdk'
import { TimeSeriesChart, type ChartSeries } from '@kineticlogic/staresdk/chart'
import { api, type SystemMetrics } from '../api/client'
import { errorMessage } from '../lib/format'
import { InfoTip } from '../components/InfoTip'

const REFRESH_MS = 15_000
const MINUTES = 60

type Point = SystemMetrics['series'][number]

const MB = 1024 * 1024
const mb = (v: number | undefined | null) => (v === undefined || v === null ? null : Math.round((v / MB) * 10) / 10)
const fmtMb = (v: number | null | undefined) => (v === null || v === undefined ? '—' : `${(v / MB).toFixed(1)} MB`)

function fmtUptime(s: number): string {
  if (s < 3600) return `${Math.floor(s / 60)}m`
  if (s < 86400) return `${Math.floor(s / 3600)}h ${Math.floor((s % 3600) / 60)}m`
  return `${Math.floor(s / 86400)}d ${Math.floor((s % 86400) / 3600)}h`
}

/** A counter per minute (0 when nothing was counted). */
const count = (pts: Point[], pick: (p: Point) => number | undefined) => pts.map((p) => pick(p) ?? 0)
/** A gauge per minute (a gap before the sampler recorded one). */
const gauge = (pts: Point[], pick: (p: Point) => number | undefined | null) => pts.map((p) => pick(p) ?? null)
const sum = (m: Record<string, number>, keys: string[]) => keys.reduce((n, k) => n + (m[k] ?? 0), 0)

function Stat({ label, value, hint }: { label: string; value: string | number; hint?: React.ReactNode }) {
  return (
    <div className="stat">
      <span className="stat-value">{typeof value === 'number' ? value.toLocaleString() : value}</span>
      <span className="stat-label">
        {label}
        {hint && <InfoTip label={label}>{hint}</InfoTip>}
      </span>
    </div>
  )
}

function Chart({ title, series, times, unit, hint }: { title: string; series: ChartSeries[]; times: number[]; unit?: string; hint?: React.ReactNode }) {
  return (
    <div className="chart">
      <h3 className="subhead">
        {title}
        {hint && <InfoTip label={title}>{hint}</InfoTip>}
      </h3>
      <TimeSeriesChart aria-label={title} times={times} series={series} unit={unit} height={130} />
    </div>
  )
}

/** Throughput, tracks, backlog and resources over the last hour, per minute. */
export default function MetricsPanels() {
  const { toast } = useToast()
  const [m, setM] = useState<SystemMetrics | null>(null)

  useEffect(() => {
    let cancelled = false
    let warned = false
    const load = () =>
      api.systemMetrics(MINUTES).then(
        (r) => {
          if (!cancelled) setM(r)
          warned = false
        },
        (e) => {
          // One toast per outage, not one per refresh.
          if (!cancelled && !warned) toast({ variant: 'error', title: 'Metrics unavailable', message: errorMessage(e) })
          warned = true
        },
      )
    load()
    const t = setInterval(load, REFRESH_MS)
    return () => {
      cancelled = true
      clearInterval(t)
    }
  }, [toast])

  // The current minute is still filling; charts end at the last complete one.
  const pts = useMemo(() => (m ? m.series.slice(0, -1) : []), [m])
  const times = useMemo(() => pts.map((p) => p.minute * 60), [pts])
  const sourceIds = useMemo(() => [...new Set(pts.flatMap((p) => Object.keys(p.emitted_by_source)))].sort(), [pts])

  if (!m) {
    return (
      <CollapsiblePanel title="Throughput" persistKey="ot.panel.throughput">
        <div className="panel-body">
          <span className="muted">LOADING…</span>
        </div>
      </CollapsiblePanel>
    )
  }
  const l = m.live
  const state = (k: string) => l.by_state[k] ?? 0

  return (
    <>
      <CollapsiblePanel title="Throughput" badge="per minute · last hour" persistKey="ot.panel.throughput">
        <div className="panel-body">
          <div className="charts">
            <Chart
              title="Ingest"
              hint="Frames: messages received from feeds. Observations: reports the pipelines passed to the engine. Dropped: records rejected by a rule, matching no mapping rule, filtered out or throttled, all on purpose. Errors: frames that did not decode and records that failed validation."
              times={times}
              series={[
                { label: 'frames', values: count(pts, (p) => p.ingest.frames) },
                { label: 'observations', values: count(pts, (p) => p.ingest.emitted), tone: 'info' },
                {
                  label: 'dropped',
                  values: count(pts, (p) => sum(p.ingest, ['rejected', 'unmatched', 'filtered', 'throttled'])),
                  tone: 'neutral',
                },
                { label: 'errors', values: count(pts, (p) => sum(p.ingest, ['decode_error', 'invalid'])), tone: 'danger' },
              ]}
            />
            <Chart
              title="Observations by source"
              times={times}
              series={sourceIds.map((id) => ({ label: id, values: count(pts, (p) => p.emitted_by_source[id]) }))}
            />
            <Chart
              title="Correlation"
              hint="Applied: observations the engine processed. New tracks: tracks it started. Paired: a source track joined a track, or two tracks merged. Proposed or split: pairings and splits put to an operator, plus splits made automatically. Ended: tracks retired when their last source track ended, or dropped after going unreported."
              times={times}
              series={[
                { label: 'applied', values: count(pts, (p) => p.engine.observations) },
                { label: 'new tracks', values: count(pts, (p) => p.engine.created), tone: 'info' },
                { label: 'paired', values: count(pts, (p) => sum(p.engine, ['paired', 'merged', 'kinematic_merged'])), tone: 'success' },
                { label: 'proposed or split', values: count(pts, (p) => sum(p.engine, ['suggested', 'split'])), tone: 'warning' },
                { label: 'ended', values: count(pts, (p) => sum(p.engine, ['retired', 'dropped'])), tone: 'neutral' },
              ]}
            />
            <Chart
              title="Output"
              hint="Tracks written: track messages published to NATS (at most one per track every few seconds, sooner when its identity changes). Deletes: withdrawals (dropped tracks, and tracks that stopped passing the output filter). Raw feed: frames sources with a raw output passed on unchanged. Errors: failed writes."
              times={times}
              series={[
                { label: 'tracks written', values: count(pts, (p) => p.writer.written) },
                { label: 'deletes', values: count(pts, (p) => p.writer.deleted), tone: 'neutral' },
                { label: 'raw feed', values: count(pts, (p) => p.ingest.raw_published), tone: 'info' },
                { label: 'errors', values: count(pts, (p) => sum(p.writer, ['write_error']) + (p.ingest.raw_error ?? 0)), tone: 'danger' },
              ]}
            />
            {pts.some((p) => p.cot && Object.keys(p.cot).length > 0) && (
              <Chart
                title="TAK output"
                hint="Events: Cursor-on-Target events sent to TAK over every output (each client of a listening output counts), refreshes included. Clients: connections at the end of the minute (TAK Servers, clients of listening outputs, multicast sockets). Dropped: connections cut for falling behind. Errors: failed connections, handshakes and writes. Settings → TAK output shows each output."
                times={times}
                series={[
                  { label: 'events', values: count(pts, (p) => p.cot?.sent) },
                  { label: 'clients', values: gauge(pts, (p) => p.cot?.clients), tone: 'info' },
                  { label: 'dropped', values: count(pts, (p) => p.cot?.dropped), tone: 'warning' },
                  { label: 'errors', values: count(pts, (p) => p.cot?.errors), tone: 'danger' },
                ]}
              />
            )}
          </div>
        </div>
      </CollapsiblePanel>

      <CollapsiblePanel title="Tracks" badge={`${l.tracks.toLocaleString()} live`} persistKey="ot.panel.trackstats">
        <div className="panel-body">
          <div className="stats">
            <Stat label="Live" value={l.tracks} />
            <Stat label="Confirmed" value={state('confirmed')} />
            <Stat label="Tentative" value={state('tentative')} />
            <Stat label="Lost" value={state('lost')} />
            <Stat label="With an entity" value={l.with_entity} hint="Live tracks resolved to a registry entity." />
            <Stat
              label="Entity differs"
              value={l.notices}
              hint="Fields, over all live tracks, where an entity's value was published in place of a different one the feed reported. Open a track to see them."
            />
            {Object.entries(l.by_domain)
              .sort((a, b) => b[1] - a[1])
              .map(([d, n]) => (
                <Stat key={d} label={d} value={n} />
              ))}
          </div>
          <div className="charts">
            <Chart
              title="Live tracks"
              times={times}
              series={[
                { label: 'live', values: gauge(pts, (p) => p.system.tracks) },
                { label: 'confirmed', values: gauge(pts, (p) => p.system.tracks_confirmed), tone: 'info' },
                { label: 'lost', values: gauge(pts, (p) => p.system.tracks_lost), tone: 'warning' },
                { label: 'with an entity', values: gauge(pts, (p) => p.system.tracks_with_entity), tone: 'neutral' },
              ]}
            />
          </div>
        </div>
      </CollapsiblePanel>

      <CollapsiblePanel title="Backlog and resources" persistKey="ot.panel.resources">
        <div className="panel-body">
          <div className="stats">
            <Stat
              label="CPU"
              value={l.cpu_milli === null ? '—' : `${(l.cpu_milli / 10).toFixed(1)}%`}
              hint="OpenTrack's CPU use over the last sample, as a share of one core: 100% is one core busy, and a multi-core host can go above it."
            />
            <Stat label="Memory" value={fmtMb(l.rss_bytes)} />
            <Stat label="Threads" value={l.threads} />
            <Stat label="Redis" value={fmtMb(l.redis_bytes)} />
            <Stat label="SQLite" value={fmtMb(l.sqlite_bytes)} />
            <Stat label="NATS stream" value={fmtMb(l.nats_bytes)} />
            <Stat
              label="Engine backlog"
              value={l.observations.lag + l.observations.pending}
              hint="Observations from enabled sources the engine has not finished: waiting (not yet read) plus in hand (read, not yet done). A number that keeps growing means the engine is not keeping up."
            />
            <Stat
              label="Writer backlog"
              value={l.outbox.lag + l.outbox.pending}
              hint="Track messages not yet published to NATS: waiting plus in hand. A number that keeps growing means NATS is slow or unreachable."
            />
            <Stat label="Uptime" value={fmtUptime(l.uptime_secs)} />
          </div>
          <div className="charts">
            <Chart
              title="Backlog"
              hint="Waiting: queued, not yet read. In hand: read but not yet acknowledged as done. Engine: observations from sources; writer: track messages to publish."
              times={times}
              series={[
                { label: 'engine waiting', values: gauge(pts, (p) => p.system.obs_lag) },
                { label: 'engine in hand', values: gauge(pts, (p) => p.system.obs_pending), tone: 'neutral' },
                { label: 'writer waiting', values: gauge(pts, (p) => p.system.outbox_lag), tone: 'info' },
                { label: 'writer in hand', values: gauge(pts, (p) => p.system.outbox_pending), tone: 'warning' },
              ]}
            />
            <Chart
              title="CPU"
              hint="OpenTrack's CPU use as a percentage of one core; above 100% it is using more than one."
              unit="%"
              times={times}
              series={[
                {
                  label: 'of one core',
                  values: gauge(pts, (p) => (p.system.cpu_milli === undefined ? null : p.system.cpu_milli / 10)),
                },
              ]}
            />
            <Chart
              title="Memory and storage"
              unit=" MB"
              times={times}
              series={[
                { label: 'OpenTrack', values: gauge(pts, (p) => mb(p.system.rss_bytes)) },
                { label: 'Redis', values: gauge(pts, (p) => mb(p.system.redis_bytes)), tone: 'info' },
                { label: 'SQLite', values: gauge(pts, (p) => mb(p.system.sqlite_bytes)), tone: 'neutral' },
                { label: 'NATS stream', values: gauge(pts, (p) => mb(p.system.nats_bytes)), tone: 'warning' },
              ]}
            />
          </div>
        </div>
      </CollapsiblePanel>
    </>
  )
}
