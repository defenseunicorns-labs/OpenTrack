import { useEffect, useState } from 'react'
import { Badge, CollapsiblePanel } from '@kineticlogic/staresdk'
import { api, type DependencyStatus, type ServerStatus } from '../api/client'
import { InfoTip } from '../components/InfoTip'

const REFRESH_MS = 5000

function Dependency({
  name,
  dep,
  detail,
  off,
  tip,
}: {
  name: string
  dep?: DependencyStatus
  detail: string
  /** Not set up (shown grey, not as a failure). */
  off?: boolean
  tip?: React.ReactNode
}) {
  return (
    <div className="dep">
      <span className="name">
        {name}
        {tip && <InfoTip label={name}>{tip}</InfoTip>}
      </span>
      {dep === undefined ? (
        <Badge color="grey" uppercase>checking</Badge>
      ) : off ? (
        <Badge color="grey" uppercase>off</Badge>
      ) : dep.ok ? (
        <Badge color="success" uppercase>up</Badge>
      ) : (
        <Badge color="danger" uppercase>down</Badge>
      )}
      <span className="mono muted">{dep && !dep.ok ? dep.error : detail}</span>
    </div>
  )
}

/** Health of every dependency the server needs, refreshed every few seconds. */
export function StatusPanel({ onStatus }: { onStatus?: (s: ServerStatus | null) => void }) {
  const [status, setStatus] = useState<ServerStatus | null>(null)
  const [unreachable, setUnreachable] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    const load = () =>
      api
        .status()
        .then((s) => {
          if (cancelled) return
          setStatus(s)
          setUnreachable(null)
          onStatus?.(s)
        })
        .catch((e: Error) => {
          if (cancelled) return
          setUnreachable(e.message)
          onStatus?.(null)
        })
    load()
    const timer = setInterval(load, REFRESH_MS)
    return () => {
      cancelled = true
      clearInterval(timer)
    }
  }, [onStatus])

  return (
    <CollapsiblePanel title="System status" persistKey="ot.panel.status">
      <div className="panel-body" style={{ gap: 0, paddingTop: 'var(--space-sm)', paddingBottom: 'var(--space-sm)' }}>
      <Dependency
        name="Control plane"
        dep={unreachable ? { ok: false, error: unreachable } : status ? { ok: true } : undefined}
        detail={status ? `v${status.version} · ${status.node_id}` : ''}
      />
      <Dependency
        name="Algorithms"
        dep={status?.algorithms ? { ok: true } : undefined}
        detail={status?.algorithms ? [status.algorithms.correlation, ...Object.values(status.algorithms.trackers)].join(' · ') : ''}
      />
      <Dependency
        name="SQLite"
        dep={status?.sqlite}
        detail={status ? [`schema v${status.sqlite.schema_version}`, status.sqlite.path].filter(Boolean).join(' · ') : ''}
      />
      <Dependency
        name="Redis"
        dep={status?.redis}
        detail={status ? `namespace ${status.redis.namespace}` : ''}
      />
      <Dependency
        name="NATS"
        dep={status?.nats}
        detail={
          status?.nats.ok
            ? [status.nats.server_name, `stream ${status.nats.stream}` + (status.nats.stream_messages != null ? ` (${status.nats.stream_messages} msgs)` : ''), `${status.nats.tracks_subject}.>`]
                .filter(Boolean)
                .join(' · ')
            : ''
        }
      />
      <Dependency
        name="Telemetry"
        dep={status ? (status.telemetry ?? { ok: true, configured: false }) : undefined}
        off={status ? !status.telemetry?.configured : false}
        detail={
          status?.telemetry?.configured
            ? ['OTLP', ...(status.telemetry.endpoints ?? [])].join(' · ')
            : 'not exported: logs on standard output only'
        }
        tip={
          <>
            Logs (the audit record included), traces and metrics sent over OpenTelemetry to your collector, set with the OTEL_EXPORTER_OTLP_* variables on each
            role. Red when a role cannot reach the collector: OpenTrack keeps working, records are dropped from the export until it recovers, standard output
            keeps every log line and the audit table every audit record. See Help → Admin guide → OpenTelemetry.
          </>
        }
      />
      </div>
    </CollapsiblePanel>
  )
}
