/**
 * A source step's samples: the first stored records as they are after that step, from the dry
 * run's trace (`POST /sources/validate` with `trace`).
 */
import type { PreviewResult, PreviewTrace, TraceFrameView } from '../api/client'

/** Records the designer follows through the pipeline. */
export const TRACED_SAMPLES = 5

/** Step names for "dropped by …", by step type (a step's id starts with its type). */
export const STAGE_LABEL: Record<string, string> = {
  input: 'Input',
  transport: 'Transport',
  decode: 'Decode',
  reject: 'Reject',
  map: 'Map',
  join: 'Identity join',
  registry: 'Entity links',
  affiliation: 'Affiliation',
  filter: 'Filter',
  tracker: 'Tracker',
  throttle: 'Throttle',
  publish: 'Publish',
}

/** One block of the pane: a sample after a stage, or at Transport a frame the samples came from. */
export interface SampleView {
  /** `#2`, or at Transport `frame 1 (samples #1–#5)`. */
  title: string
  /** What the stage passed on, ready to show: pretty JSON, or the frame's text. */
  blocks: string[]
  /** What happened to it: dropped where and why, held by a stage. */
  notes: string[]
}

const pretty = (v: unknown) => JSON.stringify(v, null, 2) ?? String(v)

/** `#1–#5` for a run of sample numbers, else `#1, #3`. */
function numbers(ns: number[]): string {
  const run = ns.every((n, i) => i === 0 || n === ns[i - 1] + 1)
  if (ns.length > 2 && run) return `#${ns[0]}–#${ns[ns.length - 1]}`
  return ns.map((n) => `#${n}`).join(', ')
}

/** The frame as received: JSON pretty-printed, text as it is, binary escaped with its length. */
function frameView(f: TraceFrameView | undefined, title: string): SampleView {
  if (!f) return { title, blocks: [], notes: ['The frame is not in the trace.'] }
  const notes = f.format === 'binary' ? [`${f.bytes} bytes, not text (shown escaped)`] : []
  if (f.truncated) notes.push(`${f.bytes.toLocaleString()} bytes: only the start is shown`)
  // A cut JSON frame comes already pretty-printed, as text.
  const block = typeof f.content === 'string' ? f.content : pretty(f.content)
  return { title, blocks: [block], notes }
}

/** A step's name for a note: its type's, numbered like its id (`Filter 2`). */
function label(stageId: string): string {
  const m = /^(.*?)(?:-(\d+))?$/.exec(stageId)
  const name = STAGE_LABEL[m?.[1] ?? stageId]
  return name ? `${name}${m?.[2] ? ` ${m[2]}` : ''}` : stageId
}

function note(stageId: string, reason: string): string {
  if (reason.startsWith('rejected:')) return reason
  return `dropped by ${label(stageId)}: ${reason}`
}

/**
 * The traced samples as they are after `stageId`. At Transport each frame they came from shows
 * once, naming its samples. Elsewhere each sample shows what the stage passed on; a sample
 * dropped at or before the stage says where and why, and one a stage held back (a tracker
 * waiting for the rest of a scan) says so.
 */
export function samplesAt(trace: PreviewTrace | undefined, stageId: string, path?: string[]): SampleView[] {
  const samples = (trace?.samples ?? []).slice(0, TRACED_SAMPLES)
  if (stageId === 'transport' || stageId === 'input') {
    const byFrame = new Map<number, number[]>()
    samples.forEach((s, i) => byFrame.set(s.frame, [...(byFrame.get(s.frame) ?? []), i + 1]))
    return [...byFrame].map(([frame, ns]) =>
      frameView(trace?.frames[frame], `frame ${frame + 1} (sample${ns.length > 1 ? 's' : ''} ${numbers(ns)})`),
    )
  }
  return samples.map((s, i) => {
    const title = `#${i + 1}`
    const at = s.stages.findIndex((x) => x.id === stageId)
    if (at < 0) return { title, blocks: [], notes: ['This step was not in the flow the preview ran.'] }
    const notes: string[] = []
    // Only the steps on the way here: a drop on another branch is not this step's.
    const before = s.stages.slice(0, at + 1).filter((x) => !path || path.includes(x.id) || x === s.stages[at])
    for (const x of before) {
      for (const r of x.dropped ?? []) notes.push(note(x.id, r))
      // Not a drop: said only at the stage that did it.
      if (x === s.stages[at]) for (const n of x.notes ?? []) notes.push(`${label(x.id)}: ${n}`)
      if (x.held) {
        notes.push(
          `held by ${label(x.id)}: waiting for the rest of its scan (the preview ran it at the end of the samples, or when a later frame completed it)`,
        )
      }
    }
    const blocks = s.stages[at].items.map(pretty)
    if (!blocks.length && !notes.length) notes.push('Nothing came out of this step.')
    return { title, blocks, notes }
  })
}

/** The counts on one line, as `frames 5 · records 5 · emitted 4`. */
export function countsLine(result: PreviewResult | null): string {
  const counts = Object.entries(result?.counts ?? {}).filter(([k]) => !k.startsWith('rejected:') && !k.startsWith('grade:'))
  return counts.map(([k, v]) => `${k} ${v}`).join(' · ')
}

/** Most samples a preview traces (crates/ot-source/src/trace.rs `MAX_SAMPLES`); the filter builder asks for them all. */
export const MAX_TRACED_SAMPLES = 10

/**
 * The observations as they reach the filter in the traced samples (what the stage before it passed
 * on), and those of them the filter dropped. With no filter in the pipeline yet, what the last
 * stage before where it goes passed on, and nothing dropped.
 */
export function filterInput(
  trace: PreviewTrace | undefined,
  id = 'filter',
  parent?: string,
): { observations: unknown[]; dropped: unknown[] } {
  const observations: unknown[] = []
  const dropped: unknown[] = []
  for (const s of trace?.samples ?? []) {
    const at = s.stages.findIndex((x) => x.id === id)
    // What reaches it: the step that feeds it.
    const before = parent
      ? s.stages.find((x) => x.id === parent)
      : at > 0
        ? s.stages[at - 1]
        : [...s.stages].reverse().find((x) => ['affiliation', 'registry', 'join', 'map'].includes(x.id))
    if (!before) continue
    observations.push(...before.items)
    if (at > 0 && s.stages[at].dropped?.length) {
      const kept = new Set(s.stages[at].items.map((i) => JSON.stringify(i)))
      dropped.push(...before.items.filter((i) => !kept.has(JSON.stringify(i))))
    }
  }
  return { observations, dropped }
}
