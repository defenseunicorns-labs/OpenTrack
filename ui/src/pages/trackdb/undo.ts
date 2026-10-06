import { useCallback } from 'react'
import { useToast } from '@kineticlogic/staresdk'
import { api, UNDOABLE_OPS, type DecisionRow } from '../../api/client'
import { errorMessage } from '../../lib/format'

/** Decisions made by the engine itself: never undone from here. */
const AUTOMATIC = ['engine', 'system']

/** A track management decision in plain words. */
export const OP_LABEL: Record<string, string> = {
  pair_tracks: 'Pair',
  unpair_tracks: 'Unpair',
  delete_track: 'Delete track',
  merge: 'Merge',
  split: 'Split',
  do_not_pair: 'Do not pair',
  create_group: 'Form group',
  update_group: 'Change group',
  group_members: 'Group members',
  dissolve_group: 'Dissolve group',
  undo: 'Undo',
  delete_history_point: 'Delete history point',
}

export const opLabel = (op: string) => OP_LABEL[op] ?? op.replace(/_/g, ' ')

/** Whether an undo of this decision can be asked for (the server may still refuse it). */
export function undoable(d: Pick<DecisionRow, 'op' | 'actor' | 'undone_by'>): boolean {
  return (UNDOABLE_OPS as readonly string[]).includes(d.op) && !AUTOMATIC.includes(d.actor) && d.undone_by == null
}

/** Track ids a decision's evidence names (tracks, from/into, group, members), in order, once each. */
export function evidenceTracks(evidence: Record<string, unknown> | null | undefined): string[] {
  if (!evidence) return []
  const out: string[] = []
  for (const k of ['tracks', 'track', 'from', 'into', 'group', 'members', 'add', 'remove', 'source_track']) {
    const v = evidence[k]
    for (const x of Array.isArray(v) ? v : [v]) if (typeof x === 'string' && x && !out.includes(x)) out.push(x)
  }
  return out
}

/**
 * Undo a decision after asking; a refusal (already undone, changed since, not undoable) is shown
 * with the server's reason. Resolves true when undone.
 */
export function useUndo() {
  const { toast, confirm } = useToast()
  return useCallback(
    async (d: { id: number; op: string }) => {
      const ok = await confirm(`${opLabel(d.op)} (decision #${d.id}) is reversed, as a new decision of its own. Tracks it retired come back.`, {
        title: `Undo #${d.id}`,
        confirmLabel: 'Undo',
      })
      if (!ok) return false
      try {
        const r = await api.undoDecision(d.id)
        toast({ variant: 'success', title: 'Undone', message: `Decision #${d.id} undone by #${String(r.decision_id ?? '?')}` })
        return true
      } catch (e) {
        toast({ variant: 'error', title: 'Not undone', message: errorMessage(e) })
        return false
      }
    },
    [toast, confirm],
  )
}
