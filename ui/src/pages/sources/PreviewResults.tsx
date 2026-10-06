import { useMemo, useState } from 'react'
import { setWorkerUrl } from 'maplibre-gl'
import maplibreWorkerUrl from 'maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url'
import { Badge, DataTable, type BadgeColor, type DataTableColumn } from '@kineticlogic/staresdk'
import { MapView } from '@kineticlogic/staresdk/map-view'
import type { PreviewObservation, PreviewResult } from '../../api/client'
import { fmtNum } from '../../lib/format'
import { affiliationColor } from '../../lib/palette'
import { PIPELINE_COUNTS_INFO } from '../../lib/sourceState'
import { InfoTip } from '../../components/InfoTip'
import { useBasemapTiles } from '../../lib/basemap'

const OUTLINES = '/world-110m.geo.json'

// MapLibre resolves its worker relative to its own module, which a bundle breaks; hand it the
// bundled worker URL before any map is created (same as OpenStare's MapLibreBasemapLayer).
setWorkerUrl(maplibreWorkerUrl)

/** Counter name → badge colour; anything else is grey. */
const COUNT_COLOR: Record<string, BadgeColor> = {
  emitted: 'blue',
  invalid: 'danger',
  decode_error: 'danger',
  rejected: 'warning',
  unmatched: 'warning',
}

const cot = (o: PreviewObservation) => o.classification?.cot_type ?? ''

const COLUMNS: DataTableColumn<PreviewObservation>[] = [
  { key: 'key', header: 'Key', mono: true, width: '18%', render: (o) => o.source_track_key, sortValue: (o) => o.source_track_key },
  { key: 'name', header: 'Name', render: (o) => o.name ?? o.callsign ?? '', sortValue: (o) => o.name ?? o.callsign },
  { key: 'type', header: 'Type', mono: true, width: '16%', render: cot, sortValue: cot },
  {
    key: 'pos',
    header: 'Position',
    mono: true,
    width: '24%',
    render: (o) => `${o.position.latitude.toFixed(4)}, ${o.position.longitude.toFixed(4)}`,
  },
  {
    key: 'spd',
    header: 'Speed m/s',
    align: 'right',
    width: '12%',
    render: (o) => fmtNum(o.kinematics?.speed_mps, 1),
    sortValue: (o) => o.kinematics?.speed_mps,
  },
]

/** Counts, errors, observations and a map for a dry run. */
export function PreviewResults({ result, showMap = true }: { result: PreviewResult; showMap?: boolean }) {
  const tiles = useBasemapTiles()
  const [selected, setSelected] = useState<string | null>(null)
  const observations = useMemo(() => result.observations ?? [], [result])
  // Several observations of one track: keep the latest per key for table and map.
  const latest = useMemo(() => {
    const m = new Map<string, PreviewObservation>()
    for (const o of observations) m.set(o.source_track_key, o)
    return [...m.values()]
  }, [observations])
  const points = useMemo(
    () =>
      latest.map((o) => ({
        id: o.source_track_key,
        latitude: o.position.latitude,
        longitude: o.position.longitude,
        color: affiliationColor(o.classification?.affiliation, o.classification?.cot_type),
        label: o.name ?? o.callsign ?? o.source_track_key,
      })),
    [latest],
  )
  const counts = Object.entries(result.counts ?? {}).filter(([k]) => !k.startsWith('rejected:') && !k.startsWith('grade:'))

  return (
    <div className="stack" style={{ gap: 8 }}>
      <div className="counts">
        {counts.length === 0 && <span className="muted">No counts yet.</span>}
        {counts.map(([k, v]) => (
          <Badge key={k} color={COUNT_COLOR[k] ?? 'grey'} size="sm">
            {k} {v}
          </Badge>
        ))}
        <span className="muted" style={{ marginLeft: 4 }}>
          {latest.length} track{latest.length === 1 ? '' : 's'}
        </span>
        <InfoTip label="Preview counts">
          What the pipeline did with the samples. {PIPELINE_COUNTS_INFO} The table and map show each track&apos;s latest observation.
          Key: the source&apos;s own track key. Type: the CoT type the mapping gave it (e.g. a-f-S is a friendly surface track).
        </InfoTip>
      </div>
      {(result.errors ?? []).slice(0, 3).map((e, i) => (
        <div key={i} className="error-text">
          {e}
        </div>
      ))}
      {showMap && (
      <MapView
        aria-label="Preview map"
        points={points}
        selectedId={selected}
        onSelect={setSelected}
        outlines={OUTLINES}
        tiles={tiles}
        fitKey={latest.length > 0 ? 'fit' : 'empty'}
        height={220}
      />
      )}
      <DataTable
        aria-label="Preview observations"
        columns={COLUMNS}
        rows={latest}
        rowKey={(o) => o.source_track_key}
        selectedKey={selected}
        onRowClick={(o) => setSelected(o.source_track_key)}
        maxHeight={240}
        empty="No observations from these samples."
      />
    </div>
  )
}
