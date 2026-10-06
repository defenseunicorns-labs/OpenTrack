import type { BadgeColor } from '@kineticlogic/staresdk'
import type { SourceRow } from '../api/client'

/** Running / starting / stopped / failing, from config plus the worker's live status. */
export function sourceState(s: SourceRow): { label: string; color: BadgeColor } {
  if (!s.enabled) return { label: 'disabled', color: 'grey' }
  if (!s.status) return { label: 'starting', color: 'warning' }
  if (s.status.not_started) return { label: 'not started', color: 'danger' }
  if (s.status.link.connected) return { label: 'running', color: 'success' }
  return { label: s.status.link.errors > 0 ? 'failing' : 'connecting', color: s.status.link.errors > 0 ? 'danger' : 'warning' }
}

/** What each source state means, for an info tip. */
export const SOURCE_STATES_INFO =
  'disabled: switched off, not running. starting: enabled, its worker has not reported yet. running: connected (or listening) and reading. ' +
  'connecting: not connected yet, no errors so far. failing: not connected and its link has hit errors; it keeps retrying. ' +
  'not started: the worker refuses to run it as configured (a listener that does not authenticate its senders, say). See the error on its Status tab.'

/** What each pipeline counter means, for an info tip. */
export const PIPELINE_COUNTS_INFO =
  'frames: messages received. decode_error: frames the codec could not read. records: what the codec decoded (a frame can hold many). ' +
  'rejected: records a Reject rule dropped. unmatched: records no mapping rule applied to. static: identity records cached to fill later reports. ' +
  'invalid: mapped records that could not become an observation (no position, a bad value, an extension field the schema does not declare). ' +
  'filtered: dropped by the Filter stage. plots: detections handed to the tracker; late_plots: arrived too late for their scan. ' +
  'tracker_error: tracker plugin runs that failed. throttled: updates held back by the Throttle stage. emitted: updates sent on to correlation.'
