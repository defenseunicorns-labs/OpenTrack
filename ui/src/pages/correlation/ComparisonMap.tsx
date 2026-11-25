import { themeFor, useTheme } from '@kineticlogic/staresdk'
import { MapView } from '@kineticlogic/staresdk/map-view'
import { setWorkerUrl } from 'maplibre-gl'
import maplibreWorkerUrl from 'maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url'
import type { SuggestionTrack } from '../../api/client'
import { useBasemapTiles } from '../../lib/basemap'

setWorkerUrl(maplibreWorkerUrl)

export default function ComparisonMap({ tracks }: { tracks: (SuggestionTrack | null)[] }) {
  const { theme } = useTheme()
  const tiles = useBasemapTiles()
  const palette = themeFor(theme)
  const points = tracks.flatMap((track, index) => {
    if (!track?.live || typeof track.latitude !== 'number' || typeof track.longitude !== 'number' ||
        !Number.isFinite(track.latitude) || !Number.isFinite(track.longitude)) return []
    return [{
      id: track.uid,
      latitude: track.latitude,
      longitude: track.longitude,
      label: track.name ?? track.track_id,
      color: index === 0 ? palette.statusInfo : palette.statusWarning,
    }]
  })
  return (
    <MapView
      aria-label="Tracks"
      points={points}
      outlines="/world-110m.geo.json"
      tiles={tiles}
      height={168}
      fitTo={{
        key: points.map((point) => point.id).join(':'),
        coordinates: points.map((point) => [point.longitude, point.latitude]),
        maxZoom: points.length === 1 ? 11 : 13,
        padding: 32,
      }}
    />
  )
}
