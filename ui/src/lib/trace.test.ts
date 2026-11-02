import { describe, expect, it } from 'vitest'
import type { PreviewTrace } from '../api/client'
import { countsLine, sampleRows, sampleTrail, samplesAt } from './trace'

// Frame 0 decodes to three records (samples #1–#3), frame 1 does not decode (#4), frame 2 is
// binary (#5), frame 3 gives #6.
const trace: PreviewTrace = {
  frames: [
    { format: 'json', bytes: 40, content: { plots: [{ n: 1 }, { n: 2 }, { n: 3 }] } },
    { format: 'text', bytes: 8, content: 'not json' },
    { format: 'binary', bytes: 2, content: '\\x01\\xff' },
    { format: 'json', bytes: 7, content: { n: 4 } },
  ],
  samples: [
    {
      frame: 0,
      stages: [
        { id: 'decode', items: [{ n: 1 }] },
        { id: 'reject', items: [{ n: 1 }] },
        { id: 'map', items: [{ source_track_key: '1' }] },
        { id: 'filter', items: [{ source_track_key: '1' }] },
        { id: 'tracker', items: [], held: true },
        { id: 'publish', items: [] },
      ],
    },
    {
      frame: 0,
      stages: [
        { id: 'decode', items: [{ n: 2 }] },
        { id: 'reject', items: [], dropped: ['rejected: no_position'] },
        { id: 'map', items: [] },
        { id: 'filter', items: [] },
        { id: 'tracker', items: [] },
        { id: 'publish', items: [] },
      ],
    },
    {
      frame: 0,
      stages: [
        { id: 'decode', items: [{ n: 3 }] },
        { id: 'reject', items: [{ n: 3 }] },
        { id: 'map', items: [{ source_track_key: '3' }] },
        { id: 'filter', items: [], dropped: ['drop_if matched'] },
        { id: 'tracker', items: [] },
        { id: 'publish', items: [] },
      ],
    },
    { frame: 1, stages: [{ id: 'decode', items: [], dropped: ['decode error: bad'] }, { id: 'map', items: [] }, { id: 'publish', items: [] }] },
    { frame: 2, stages: [] },
    { frame: 3, stages: [{ id: 'decode', items: [{ n: 4 }] }, { id: 'publish', items: [{ op: 'upsert' }] }] },
  ],
}

describe('samplesAt', () => {
  it('shows each frame the samples came from once at Transport', () => {
    const s = samplesAt(trace, 'transport')
    expect(s.map((x) => x.title)).toEqual(['frame 1 (samples #1–#3)', 'frame 2 (sample #4)', 'frame 3 (sample #5)'])
    expect(s[0].blocks).toEqual([JSON.stringify(trace.frames[0].content, null, 2)])
    expect(s[1].blocks).toEqual(['not json'])
    expect(s[2].blocks).toEqual(['\\x01\\xff'])
    expect(s[2].notes[0]).toMatch(/^2 bytes/)
  })

  it('follows each record, one sample each, to the selected stage', () => {
    const decode = samplesAt(trace, 'decode')
    expect(decode.map((x) => x.title)).toEqual(['#1', '#2', '#3', '#4', '#5'])
    expect(decode[1].blocks).toEqual([JSON.stringify({ n: 2 }, null, 2)])

    const map = samplesAt(trace, 'map')
    expect(map[0].blocks).toEqual([JSON.stringify({ source_track_key: '1' }, null, 2)])
    expect(map[1]).toEqual({ title: '#2', blocks: [], notes: ['rejected: no_position'] })
    expect(map[3].notes).toEqual(['dropped by Decode: decode error: bad'])

    const filter = samplesAt(trace, 'filter')
    expect(filter[2].notes).toEqual(['dropped by Filter: drop_if matched'])
    // Not a stage of this sample's (older) trace.
    expect(filter[3].notes[0]).toMatch(/not in the flow/)
  })

  it('says when a stage holds a sample, and when nothing came out', () => {
    const tracker = samplesAt(trace, 'tracker')
    expect(tracker[0].blocks).toEqual([])
    expect(tracker[0].notes.at(-1)).toMatch(/^held by Tracker/)
    const publish = samplesAt(trace, 'publish')
    expect(publish[0].notes.some((n) => n.startsWith('held by Tracker'))).toBe(true)
    const one: PreviewTrace = { frames: trace.frames, samples: [{ frame: 0, stages: [{ id: 'map', items: [] }] }] }
    expect(samplesAt(one, 'map')[0].notes).toEqual(['Nothing came out of this step.'])
  })

  it('shows at most five samples, and none without a trace', () => {
    expect(samplesAt(trace, 'publish')).toHaveLength(5)
    expect(samplesAt(undefined, 'map')).toEqual([])
    expect(samplesAt(undefined, 'transport')).toEqual([])
  })
})

describe('countsLine', () => {
  it('puts the counts on one line without the per-reason ones', () => {
    expect(countsLine({ valid: true, counts: { frames: 5, records: 7, 'rejected:no_position': 1, 'grade:none': 4, emitted: 4 } })).toBe(
      'frames 5 · records 7 · emitted 4',
    )
    expect(countsLine(null)).toBe('')
  })
})

describe('big frames and stage notes', () => {
  const big: PreviewTrace = {
    frames: [{ format: 'json', bytes: 91838, truncated: true, content: '{\n  "ac": [\n    {\n      "hex": "ae5662"' }],
    samples: [
      {
        frame: 0,
        stages: [
          { id: 'decode', items: [{ hex: 'ae5662' }] },
          { id: 'map', items: [{ kind: 'static' }, { callsign: 'SURF21' }] },
          { id: 'join', items: [{ callsign: 'SURF21' }], notes: ['kept as the static identity of ae5662, to fill its later reports'] },
          { id: 'publish', items: [{ op: 'upsert' }] },
        ],
      },
    ],
  }

  it('shows a cut JSON frame as its pretty text, saying only the start is there', () => {
    const [f] = samplesAt(big, 'transport')
    expect(f.blocks).toEqual(['{\n  "ac": [\n    {\n      "hex": "ae5662"'])
    expect(f.notes).toEqual(['91,838 bytes: only the start is shown'])
  })

  it('says a stage note at that stage only, never as a drop', () => {
    const [atJoin] = samplesAt(big, 'join')
    expect(atJoin.notes).toEqual(['Identity join: kept as the static identity of ae5662, to fill its later reports'])
    const [atPublish] = samplesAt(big, 'publish')
    expect(atPublish.notes).toEqual([])
    expect(atPublish.blocks).toHaveLength(1)
  })
})

describe('sampleRows', () => {
  const trace = {
    frames: [{ format: 'json' as const, bytes: 10, content: {} }],
    samples: [
      {
        frame: 0,
        stages: [
          { id: 'input', items: [{ n: 1 }] },
          { id: 'map', items: [{ key: '1', lat: 1 }] },
          { id: 'filter', items: [{ key: '1', lat: 1 }] },
        ],
      },
      {
        frame: 0,
        stages: [
          { id: 'input', items: [{ n: 2 }] },
          { id: 'map', items: [{ key: '2', lat: 40 }] },
          { id: 'filter', items: [], dropped: ['drop_if matched'] },
        ],
      },
      { frame: 0, stages: [{ id: 'input', items: [], dropped: ['decode error: bad'] }, { id: 'map', items: [] }, { id: 'filter', items: [] }] },
    ],
  }

  it('puts each sample beside what reached the step, and says why one did not come out', () => {
    const rows = sampleRows(trace, 'filter', 'map', ['input', 'map', 'filter'])
    expect(rows[0]).toEqual({ id: '0-0', label: '#1', value: { key: '1', lat: 1 }, before: { key: '1', lat: 1 } })
    expect(rows[1]).toEqual({ id: '1', label: '#2', dropped: 'dropped by Filter: drop_if matched' })
    expect(rows[2].dropped).toBe('dropped by Input: decode error: bad')
  })

  it("shows the input's records with no step before, and its decode errors", () => {
    const rows = sampleRows(trace, 'input', undefined, ['input'])
    expect(rows.map((r) => r.value ?? r.dropped)).toEqual([{ n: 1 }, { n: 2 }, 'decode error: bad'])
  })
})

describe('sampleTrail', () => {
  it('marks the steps a sample passed and the one that dropped it', () => {
    const trace = {
      frames: [],
      samples: [
        {
          frame: 0,
          stages: [
            { id: 'input', items: [{ n: 1 }] },
            { id: 'map', items: [{ k: 1 }] },
            { id: 'f', items: [], dropped: ['drop_if matched'] },
            { id: 'raw', items: [{ k: 1 }] },
            { id: 'out', items: [] },
            { id: 'publish', items: [] },
          ],
        },
      ],
    }
    expect(sampleTrail(trace, 0)).toEqual({
      input: { state: 'passed' },
      map: { state: 'passed' },
      f: { state: 'dropped', note: 'drop_if matched' },
      raw: { state: 'passed' },
    })
    expect(sampleTrail(trace, 5)).toEqual({})
  })
})
