import { lazy, Suspense, type ReactNode } from 'react'
import { TbCheck, TbX } from 'react-icons/tb'
import { Badge, Button, Disclosure } from '@kineticlogic/staresdk'
import type { DecisionRow, Suggestion, SuggestionTrack } from '../../api/client'
import { InfoTip } from '../../components/InfoTip'
import { fmtTime } from '../../lib/format'
import { evidenceTracks } from '../trackdb/undo'

const ComparisonMap = lazy(() => import('./ComparisonMap'))

export const trackName = (track: SuggestionTrack | null) => track?.name ?? track?.track_id ?? track?.uid ?? ''

type InspectTrack = (track: SuggestionTrack, trigger: HTMLButtonElement) => void

function TrackSummary({ track, color, onInspect }: { track: SuggestionTrack; color: 'blue' | 'warning'; onInspect: InspectTrack }) {
  const number = track.track_id || track.uid
  return (
    <div className="correlation-track">
      <strong>{trackName(track)}</strong>
      <Button
        size="sm"
        variant="ghost"
        aria-haspopup="dialog"
        style={{ height: 'auto', padding: 'var(--space-xs)', margin: 'calc(-1 * var(--space-xs))', textTransform: 'none' }}
        onClick={(event) => onInspect(track, event.currentTarget)}
      >
        <Badge color={color} size="sm">{number}</Badge>
      </Button>
      <span className="muted">{track.live ? track.state : 'No longer live'}</span>
      {track.published === false && <span className="muted">not published</span>}
      {!!track.sources?.length && (
        <ul className="correlation-sources mono">
          {track.sources.map((source) => <li key={source}>{source}</li>)}
        </ul>
      )}
    </div>
  )
}

type Evidence = Record<string, unknown>

function EvidenceDetails({ evidence, label = 'Evidence', overview }: {
  evidence: Evidence | null
  label?: string
  overview?: ReactNode
}) {
  const p = evidence?.probability
  const threshold = evidence?.pair_probability ?? evidence?.split_probability
  const comparisons = evidence?.comparisons
  const outside = evidence?.outside_gate
  const emitterMatch = evidence?.rule === 'area-emitter'
  const last = evidence?.last
  const distance = last && typeof last === 'object' && 'distance_m' in last ? last.distance_m : null
  return (
    <div className="correlation-evidence">
      <InfoTip label="Evidence">
        {overview ?? 'Open Evidence to see the details OpenTrack saved.'}
        {typeof p === 'number' && Number.isFinite(p) && (
          <div>
            {emitterMatch
              ? `Out of the tracks that matched, this one got ${formatProbability(p)} of the total match score.`
              : `OpenTrack estimated a ${formatProbability(p)} chance that the reports show the same object.`}
            {!emitterMatch && typeof threshold === 'number' && Number.isFinite(threshold) && (
              evidence?.split_probability === undefined
                ? ` Pairing required a score of at least ${formatProbability(threshold)}.`
                : ` A split required a score of ${formatProbability(threshold)} or less.`
            )}
          </div>
        )}
        {typeof comparisons === 'number' && (
          <div>
            OpenTrack checked the reports against each other {comparisons} {comparisons === 1 ? 'time' : 'times'}.
            {typeof outside === 'number' && ` In ${outside} of those checks, the reports were too different to agree.`}
          </div>
        )}
        {typeof distance === 'number' && Number.isFinite(distance) && (
          <div>The tracks were {Math.round(distance).toLocaleString()} meters apart in the last check.</div>
        )}
      </InfoTip>
      <Disclosure label="Evidence">
        <pre className="mono evidence" aria-label={label}>{JSON.stringify(evidence ?? {}, null, 2)}</pre>
      </Disclosure>
    </div>
  )
}

const formatProbability = (value: number) => `${Number((value * 100).toFixed(2))}%`

export function SuggestionDetails({
  suggestion: s,
  canManage,
  busy,
  onDecide,
  onInspect,
  onTune,
}: {
  suggestion: Suggestion
  canManage: boolean
  busy: boolean
  onDecide: (decision: 'accept' | 'reject') => void
  onInspect: InspectTrack
  onTune: () => void
}) {
  const pair = s.kind === 'pair'
  const source = s.source_track
  const hasPosition = [s.a, s.b].some((track) => (
    track?.live && Number.isFinite(track.latitude) && Number.isFinite(track.longitude)
  ))
  const overview = s.evidence.rule === 'area-emitter'
    ? 'The area report and this track share a signal ID. OpenTrack found this track to be the best match.'
    : pair
      ? 'OpenTrack thinks these two tracks may show the same object.'
      : 'One source’s reports stopped lining up with the other reports for this track.'
  return (
    <div className="panel-body">
      <div className="correlation-tracks" aria-label="Tracks">
        <TrackSummary track={s.a} color="blue" onInspect={onInspect} />
        {s.b ? <TrackSummary track={s.b} color="warning" onInspect={onInspect} /> : source && (
          <div className="correlation-track"><strong className="mono">{source}</strong></div>
        )}
      </div>
      {hasPosition && (
        <Suspense fallback={<span className="muted">LOADING…</span>}>
          <ComparisonMap tracks={[s.a, s.b]} />
        </Suspense>
      )}
      <EvidenceDetails evidence={s.evidence} overview={
        <>
          <div>{overview}</div>
          {s.status === 'expired' && <div>The tracks changed or ended, so this suggestion expired.</div>}
        </>
      } />
      {s.status === 'open' && (
        <div className="correlation-outcomes">
          <div>
            <Button size="sm" variant="primary" icon={<TbCheck />} disabled={!canManage || busy} onClick={() => onDecide('accept')}>Accept</Button>
            <InfoTip label="Accept">
              {pair
                ? `Combine the two tracks. ${trackName(s.a)} keeps track ID ${s.a.track_id}. ${s.b ? trackName(s.b) : s.track_b ?? 'The other track'} stops being shared as a separate track.`
                : `Move ${source ?? 'the source’s reports'} to a new track. OpenTrack remembers that the two tracks show different objects.`}
            </InfoTip>
          </div>
          <div>
            <Button size="sm" variant="secondary" icon={<TbX />} disabled={!canManage || busy} onClick={() => onDecide('reject')}>Reject</Button>
            <InfoTip label="Reject">
              {pair
                ? 'Keep the tracks separate. OpenTrack remembers that they show different objects and will skip this pairing in the future.'
                : 'Keep the reports on their current track. OpenTrack waits 30 minutes before suggesting this split again.'}
            </InfoTip>
          </div>
        </div>
      )}
      <div><Button size="sm" variant="ghost" onClick={onTune}>Use in Settings</Button></div>
    </div>
  )
}

const DECISION_SUMMARY: Record<string, string> = {
  pair: 'Reports from one source joined an existing track.',
  merge: 'Two tracks became one.',
  split: 'Reports from one source moved to their own track.',
  do_not_pair: 'OpenTrack saved that these tracks show different objects so they stay separate.',
  reject_split: 'An operator kept the reports on their current track.',
  end_source_track: 'A source stopped reporting its track.',
  retire_system_track: 'The track ended after all its sources stopped reporting it.',
  correlation_settings: 'Correlation settings were saved.',
}

export function DecisionDetails({ decision: d, onTune }: { decision: DecisionRow; onTune?: () => void }) {
  const tracks = evidenceTracks(d.evidence)
  return (
    <div className="panel-body">
      <div className="muted">{d.actor} · {fmtTime(d.at_ms)}</div>
      {!!tracks.length && <div className="mono">{tracks.join(', ')}</div>}
      {d.undone_by != null && <span className="muted">undone by #{d.undone_by}</span>}
      {d.undoes != null && <span className="muted">undoes #{d.undoes}</span>}
      <EvidenceDetails evidence={d.evidence} label={`Evidence of decision ${d.id}`} overview={DECISION_SUMMARY[d.op]} />
      {onTune && <div><Button size="sm" variant="ghost" onClick={onTune}>Use in Settings</Button></div>}
    </div>
  )
}
