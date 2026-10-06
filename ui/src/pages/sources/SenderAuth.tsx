import { Badge, Label, Toggle } from '@kineticlogic/staresdk'
import type { SourceSpec } from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { sendersAuthenticated } from '../../lib/senderAuth'

/** A source runs with unauthenticated senders accepted: the warning badge for lists. */
export function UnauthenticatedBadge({ spec }: { spec: SourceSpec }) {
  if (spec.unauthenticated !== 'accepted') return null
  return (
    <Badge color="danger" size="sm" uppercase title="Listens without authenticating its senders; the risk is accepted on the source.">
      unauthenticated
    </Badge>
  )
}

/**
 * The per-source risk acceptance for a listener that does not authenticate its senders. Shown only where it applies;
 * a listener that authenticates must have it cleared, since the server refuses the flag where it is not needed.
 */
export function SenderAuth({ spec, onChange }: { spec: SourceSpec; onChange: (s: SourceSpec) => void }) {
  const auth = sendersAuthenticated(spec.transport)
  const accepted = spec.unauthenticated === 'accepted'
  if (auth !== false && !accepted) return null
  const setAccepted = (on: boolean) => {
    const next = { ...spec }
    if (on) next.unauthenticated = 'accepted'
    else delete next.unauthenticated
    onChange(next)
  }
  const udp = spec.transport.type === 'udp'
  return (
    <div className="field wide" role="group" aria-label="Unauthenticated senders">
      <div className="row-label">
        <Label size="sm">
          <span className="error-text">Accept unauthenticated senders</span>
        </Label>
        <InfoTip label="Unauthenticated senders">
          A listening source must authenticate who sends to it: mutual TLS (a client CA, certificates required) for a TCP server; mutual
          TLS or a bearer token for a gRPC server. {udp ? 'UDP, multicast included, cannot carry TLS, so a UDP source needs this. ' : ''}
          On: the source runs without that, and anyone who can reach its port can feed it tracks. It records that the risk is accepted,
          which needs the authorising official&apos;s acceptance (see the hardening checklist); the change is kept in the decision log.
          {auth === true ? ' This source now authenticates its senders: switch this off to save.' : ''}
        </InfoTip>
      </div>
      <div className="row" style={{ gap: 8 }}>
        <Toggle size="sm" value={accepted} onChange={setAccepted} aria-label="Accept unauthenticated senders" />
        {accepted ? (
          <Badge color="danger" size="sm" uppercase>
            risk accepted
          </Badge>
        ) : (
          <span className="error-text">{udp ? 'Required for UDP.' : 'Required unless senders authenticate.'}</span>
        )}
      </div>
    </div>
  )
}
