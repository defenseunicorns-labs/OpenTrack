import { useEffect, useEffectEvent, useRef, useState, type CSSProperties } from 'react'
import { TbAdjustmentsHorizontal, TbMap, TbMapOff } from 'react-icons/tb'
import { AttributionControl, Map as MapLibreMap, Popup, type GeoJSONSource } from 'maplibre-gl'
import { Button, ButtonPalette, Label, Select, Slider, themeFor, Toggle, useTheme } from '@kineticlogic/staresdk'
import { EVIDENCE_COLOR, UNCERTAINTY_COLOR } from '../../lib/palette'
import {
  coordinateBounds,
  linesToGeoJSON,
  pointBounds,
  pointsToGeoJSON,
  type MapFitTo,
  type MapLine,
  type MapPoint,
  type MapTiles,
} from '@kineticlogic/staresdk/map-view'

type MarkerSize = 'compact' | 'standard' | 'large'

interface MapDisplay {
  basemap: boolean
  dim: number
  highContrast: boolean
  markerSize: MarkerSize
  evidence: boolean
  sensors: boolean
  uncertainty: boolean
  focus: boolean
  labels: boolean
}

/** A track on the map; `text` is its label on the map (the popup shows `label`). */
export type TrackMapPoint = MapPoint & { text?: string }

interface TrackMapProps {
  points: TrackMapPoint[]
  sensorPoints?: MapPoint[]
  lines?: MapLine[]
  evidenceLines?: MapLine[]
  uncertaintyLines?: MapLine[]
  selectedId?: string | null
  onSelect?: (id: string | null) => void
  outlines?: Parameters<GeoJSONSource['setData']>[0]
  tiles?: MapTiles
  fitKey?: string | number
  fitTo?: MapFitTo
  height?: number | string
  'aria-label': string
  style?: CSSProperties
}

/** The label font: its glyph ranges are vendored by scripts/vendor-map-glyphs.sh. */
const LABEL_FONT = 'Noto Sans Medium'
const STORAGE_KEY = 'ot.map.track.display'
const DEFAULT_DISPLAY: MapDisplay = {
  basemap: true,
  dim: 25,
  highContrast: true,
  markerSize: 'standard',
  evidence: true,
  sensors: true,
  uncertainty: true,
  focus: true,
  labels: true,
}
const EMPTY = { type: 'FeatureCollection' as const, features: [] }

function loadDisplay(): MapDisplay {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}') as Partial<MapDisplay>
    return {
      ...DEFAULT_DISPLAY,
      ...saved,
      dim: Number.isFinite(saved.dim) ? Math.max(0, Math.min(60, Number(saved.dim))) : DEFAULT_DISPLAY.dim,
      markerSize: ['compact', 'standard', 'large'].includes(saved.markerSize ?? '') ? saved.markerSize! : DEFAULT_DISPLAY.markerSize,
    }
  } catch {
    return DEFAULT_DISPLAY
  }
}

const MARKER_SIZES = [
  { value: 'compact', label: 'Compact' },
  { value: 'standard', label: 'Standard' },
  { value: 'large', label: 'Large' },
]

const pointRadius = (size: MarkerSize) => ({ compact: 4, standard: 6, large: 8 })[size]

export function TrackMap({
  points,
  sensorPoints = [],
  lines = [],
  evidenceLines = [],
  uncertaintyLines = [],
  selectedId,
  onSelect,
  outlines,
  tiles,
  fitKey,
  fitTo,
  height = 240,
  'aria-label': ariaLabel,
  style,
}: TrackMapProps) {
  const { theme } = useTheme()
  const palette = themeFor(theme)
  const host = useRef<HTMLDivElement>(null)
  const map = useRef<MapLibreMap | null>(null)
  const fitted = useRef<string | number | null>(null)
  const fittedTo = useRef<string | number | null>(null)
  const paletteRef = useRef(palette)
  const [ready, setReady] = useState(false)
  const [display, setDisplay] = useState(loadDisplay)
  const [menuOpen, setMenuOpen] = useState(false)
  const select = useEffectEvent((id: string | null) => onSelect?.(id))

  const updateDisplay = (next: Partial<MapDisplay>) => {
    setDisplay((current) => {
      const updated = { ...current, ...next }
      try {
        localStorage.setItem(STORAGE_KEY, JSON.stringify(updated))
      } catch {
        /* storage unavailable: the choice lasts this page load */
      }
      return updated
    })
  }

  useEffect(() => {
    if (!host.current) return
    const m = new MapLibreMap({
      container: host.current,
      // Labels are drawn from glyph ranges this server serves (ui/public/map-fonts).
      style: { version: 8, glyphs: `${window.location.origin}/map-fonts/{fontstack}/{range}.pbf`, sources: {}, layers: [] },
      center: [0, 20],
      zoom: 0.6,
      attributionControl: false,
      renderWorldCopies: false,
      dragRotate: false,
      pitchWithRotate: false,
    })
    m.touchZoomRotate.disableRotation()
    m.on('load', () => {
      const b = paletteRef.current.offlineBasemap
      m.addSource('outlines', { type: 'geojson', data: EMPTY })
      m.addSource('lines', { type: 'geojson', data: EMPTY })
      m.addSource('evidence', { type: 'geojson', data: EMPTY })
      m.addSource('uncertainty', { type: 'geojson', data: EMPTY })
      m.addSource('sensors', { type: 'geojson', data: EMPTY })
      m.addSource('points', { type: 'geojson', data: EMPTY })
      m.addLayer({ id: 'background', type: 'background', paint: { 'background-color': b.ocean } })
      m.addLayer({ id: 'land', type: 'fill', source: 'outlines', paint: { 'fill-color': b.landFill, 'fill-opacity': b.landFillOpacity } })
      m.addLayer({ id: 'borders', type: 'line', source: 'outlines', paint: { 'line-color': b.countryBorder, 'line-width': 0.6 } })

      const addLines = (source: string, id: string, dashed: boolean, casing: boolean) => {
        m.addLayer({
          id,
          type: 'line',
          source,
          filter: dashed ? ['get', 'dashed'] : ['!', ['get', 'dashed']],
          layout: { 'line-join': 'round', 'line-cap': dashed ? 'butt' : 'round' },
          paint: {
            'line-color': casing ? paletteRef.current.colorBgPrimary : ['get', 'color'],
            'line-width': casing ? ['+', ['get', 'width'], 4] : ['get', 'width'],
            'line-opacity': casing ? ['*', ['get', 'opacity'], 0.95] : ['get', 'opacity'],
            ...(dashed ? { 'line-dasharray': [2, 2] } : {}),
          },
        })
      }
      for (const source of ['lines', 'evidence', 'uncertainty']) {
        addLines(source, `${source}-case`, false, true)
        addLines(source, `${source}-dashed-case`, true, true)
        addLines(source, source, false, false)
        addLines(source, `${source}-dashed`, true, false)
      }

      m.addLayer({
        id: 'sensors',
        type: 'circle',
        source: 'sensors',
        paint: {
          'circle-color': ['get', 'color'],
          'circle-radius': 5,
          'circle-stroke-width': 2,
          'circle-stroke-color': paletteRef.current.colorBgPrimary,
        },
      })
      m.addLayer({
        id: 'selected-halo',
        type: 'circle',
        source: 'points',
        filter: ['get', 'selected'],
        paint: {
          'circle-color': paletteRef.current.colorTextPrimary,
          'circle-radius': 10,
          'circle-stroke-width': 2,
          'circle-stroke-color': paletteRef.current.colorBgPrimary,
        },
      })
      m.addLayer({
        id: 'points',
        type: 'circle',
        source: 'points',
        paint: {
          'circle-color': ['get', 'color'],
          'circle-radius': ['case', ['get', 'selected'], 7, 6],
          'circle-stroke-width': 2,
          'circle-stroke-color': paletteRef.current.colorBgPrimary,
        },
      })
      // Track labels, decluttered: a label that would collide with one already placed is left
      // out (more fit as you zoom in). The selected track's label is its own layer, placed
      // first and always shown, so the others make room for it.
      const label = (id: string, selected: boolean) =>
        m.addLayer({
          id,
          type: 'symbol',
          source: 'points',
          filter: selected ? ['get', 'selected'] : ['!', ['get', 'selected']],
          layout: {
            'text-field': ['get', 'text'],
            'text-font': [LABEL_FONT],
            'text-size': selected ? 12 : 11,
            'text-variable-anchor': ['left', 'right', 'top', 'bottom'],
            'text-radial-offset': 0.9,
            'text-justify': 'auto',
            'text-max-width': 12,
            'text-allow-overlap': selected,
            'text-padding': 3,
          },
          paint: {
            'text-color': paletteRef.current.colorTextPrimary,
            'text-halo-color': paletteRef.current.colorBgPrimary,
            'text-halo-width': 1.5,
          },
        })
      label('labels', false)
      label('labels-selected', true)
      setReady(true)
    })
    m.on('click', (event) => {
      const hit = m.queryRenderedFeatures(event.point, { layers: ['points'] })[0]
      select(hit ? String(hit.properties?.id) : null)
    })
    const popup = new Popup({ closeButton: false, closeOnClick: false, offset: 10, className: 'ui-map-view__popup' })
    m.on('mousemove', 'points', (event) => {
      const feature = event.features?.[0]
      if (!feature) return
      m.getCanvas().style.cursor = 'pointer'
      popup.setLngLat(event.lngLat).setText(String(feature.properties?.label ?? '')).addTo(m)
    })
    m.on('mouseleave', 'points', () => {
      m.getCanvas().style.cursor = ''
      popup.remove()
    })
    map.current = m
    return () => {
      popup.remove()
      m.remove()
      map.current = null
    }
  }, [])

  useEffect(() => {
    const m = map.current
    if (!m || !ready) return
    const b = palette.offlineBasemap
    m.setPaintProperty('background', 'background-color', b.ocean)
    m.setPaintProperty('land', 'fill-color', b.landFill)
    m.setPaintProperty('land', 'fill-opacity', b.landFillOpacity)
    m.setPaintProperty('borders', 'line-color', b.countryBorder)
    // Outlines and the selection halo follow the theme: dark casing on dark, light on light.
    for (const id of ['points', 'sensors', 'selected-halo']) m.setPaintProperty(id, 'circle-stroke-color', palette.colorBgPrimary)
    m.setPaintProperty('selected-halo', 'circle-color', palette.colorTextPrimary)
    for (const id of ['labels', 'labels-selected']) {
      m.setPaintProperty(id, 'text-color', palette.colorTextPrimary)
      m.setPaintProperty(id, 'text-halo-color', palette.colorBgPrimary)
    }
    for (const source of ['lines', 'evidence', 'uncertainty']) {
      m.setPaintProperty(`${source}-case`, 'line-color', palette.colorBgPrimary)
      m.setPaintProperty(`${source}-dashed-case`, 'line-color', palette.colorBgPrimary)
    }
  }, [ready, palette])

  useEffect(() => {
    const m = map.current
    if (!m || !ready) return
    ;(m.getSource('outlines') as GeoJSONSource).setData(outlines ?? EMPTY)
  }, [ready, outlines])

  const tileUrl = tiles?.url.trim() || ''
  const tileAttribution = tiles?.attribution?.trim() || ''
  const tileSize = tiles?.tileSize ?? 256
  const tileMaxZoom = tiles?.maxZoom ?? 19
  useEffect(() => {
    const m = map.current
    if (!m || !ready || !tileUrl) return
    m.addSource('basemap', { type: 'raster', tiles: [tileUrl], tileSize, maxzoom: tileMaxZoom, attribution: tileAttribution || undefined })
    m.addLayer({ id: 'basemap', type: 'raster', source: 'basemap' }, 'land')
    const credit = tileAttribution ? new AttributionControl({ compact: true }) : null
    if (credit) m.addControl(credit, 'bottom-right')
    return () => {
      if (map.current !== m) return
      if (credit) m.removeControl(credit)
      m.removeLayer('basemap')
      m.removeSource('basemap')
    }
  }, [ready, tileUrl, tileAttribution, tileSize, tileMaxZoom])

  useEffect(() => {
    const m = map.current
    if (!m || !ready) return
    const offlineVisibility = display.basemap && !tileUrl ? 'visible' : 'none'
    m.setLayoutProperty('land', 'visibility', offlineVisibility)
    m.setLayoutProperty('borders', 'visibility', offlineVisibility)
    if (m.getLayer('basemap')) {
      m.setLayoutProperty('basemap', 'visibility', display.basemap ? 'visible' : 'none')
      m.setPaintProperty('basemap', 'raster-brightness-max', 1 - display.dim / 100)
      m.setPaintProperty('basemap', 'raster-saturation', -display.dim / 100)
    }
  }, [ready, tileUrl, display.basemap, display.dim])

  useEffect(() => {
    const m = map.current
    if (!m || !ready) return
    const radius = pointRadius(display.markerSize)
    m.setPaintProperty('points', 'circle-radius', ['case', ['get', 'selected'], radius + 2, radius])
    m.setPaintProperty('points', 'circle-stroke-width', display.highContrast ? 2.5 : 1)
    m.setPaintProperty('points', 'circle-opacity', display.focus && selectedId ? ['case', ['get', 'selected'], 1, 0.35] : 1)
    m.setPaintProperty('selected-halo', 'circle-radius', radius + 5)
    m.setLayoutProperty('selected-halo', 'visibility', display.highContrast ? 'visible' : 'none')
    m.setLayoutProperty('sensors', 'visibility', display.sensors ? 'visible' : 'none')
    for (const id of ['labels', 'labels-selected']) m.setLayoutProperty(id, 'visibility', display.labels ? 'visible' : 'none')
    m.setPaintProperty('labels', 'text-opacity', display.focus && selectedId ? 0.35 : 1)
    for (const source of ['lines', 'evidence', 'uncertainty']) {
      const visible = source === 'evidence' ? display.evidence : source === 'uncertainty' ? display.uncertainty : true
      m.setLayoutProperty(source, 'visibility', visible ? 'visible' : 'none')
      m.setLayoutProperty(`${source}-dashed`, 'visibility', visible ? 'visible' : 'none')
      m.setLayoutProperty(`${source}-case`, 'visibility', visible && display.highContrast ? 'visible' : 'none')
      m.setLayoutProperty(`${source}-dashed-case`, 'visibility', visible && display.highContrast ? 'visible' : 'none')
    }
  }, [ready, display, selectedId])

  useEffect(() => {
    const m = map.current
    if (!m || !ready) return
    const text = new Map(points.map((p) => [p.id, p.text ?? '']))
    const data = pointsToGeoJSON(points, selectedId, palette.statusInfo)
    for (const f of data.features) f.properties = { ...f.properties, text: text.get(String(f.properties?.id)) ?? '' }
    ;(m.getSource('points') as GeoJSONSource).setData(data)
    ;(m.getSource('sensors') as GeoJSONSource).setData(pointsToGeoJSON(sensorPoints, null, EVIDENCE_COLOR))
    if (fitted.current !== fitKey || fitted.current === null) {
      const bounds = pointBounds(points)
      if (bounds) {
        m.fitBounds(bounds, { padding: 24, maxZoom: 9, duration: 0 })
        fitted.current = fitKey ?? ''
      }
    }
  }, [ready, points, sensorPoints, selectedId, fitKey, palette.statusInfo])

  useEffect(() => {
    const m = map.current
    if (!m || !ready) return
    ;(m.getSource('lines') as GeoJSONSource).setData(linesToGeoJSON(lines, palette.statusInfo))
    ;(m.getSource('evidence') as GeoJSONSource).setData(linesToGeoJSON(evidenceLines, EVIDENCE_COLOR))
    ;(m.getSource('uncertainty') as GeoJSONSource).setData(linesToGeoJSON(uncertaintyLines, UNCERTAINTY_COLOR))
  }, [ready, lines, evidenceLines, uncertaintyLines, palette.statusInfo])

  useEffect(() => {
    const m = map.current
    if (!m || !ready || !fitTo || fittedTo.current === fitTo.key) return
    const bounds = coordinateBounds(fitTo.coordinates)
    if (!bounds) return
    fittedTo.current = fitTo.key
    m.fitBounds(bounds, { padding: fitTo.padding ?? 32, maxZoom: fitTo.maxZoom ?? 12, duration: 600 })
  }, [ready, fitTo])

  return (
    <div className="track-map" style={{ height, ...style }}>
      <div ref={host} role="region" aria-label={ariaLabel} className="ui-map-view track-map__canvas" />
      {/* Map buttons on a palette (its glass surface keeps them legible over the map), as OpenStare's map controls. */}
      <div className="track-map__controls">
        <ButtonPalette orientation="vertical" ariaLabel="Map controls" gap={4}>
          <Button
            size="xs"
            variant="ghost"
            icon={display.basemap ? <TbMapOff /> : <TbMap />}
            aria-label={display.basemap ? 'Hide basemap' : 'Show basemap'}
            title={display.basemap ? 'Hide basemap' : 'Show basemap'}
            onClick={() => updateDisplay({ basemap: !display.basemap })}
          />
          <Button
            size="xs"
            variant="ghost"
            icon={<TbAdjustmentsHorizontal />}
            aria-label="Map display options"
            title="Map display options"
            aria-expanded={menuOpen}
            active={menuOpen}
            onClick={() => setMenuOpen((open) => !open)}
          />
        </ButtonPalette>
      </div>
      {menuOpen && (
        <div className="track-map__menu" role="dialog" aria-label="Map display options">
          <h4 className="subhead">Map display</h4>
          <div className="track-map__row track-map__row--stacked">
            <Label size="sm">Basemap dimming</Label>
            <Slider aria-label="Basemap dimming" min={0} max={60} step={5} value={display.dim} onChange={(dim) => updateDisplay({ dim })} formatValue={(v) => `${v}%`} />
          </div>
          <div className="track-map__row">
            <Label size="sm">Track size</Label>
            <Select
              ariaLabel="Track size"
              options={MARKER_SIZES}
              value={display.markerSize}
              onChange={(v) => v && updateDisplay({ markerSize: v as MarkerSize })}
              style={{ width: 120 }}
            />
          </div>
          <MapToggle label="Track labels" checked={display.labels} onChange={(labels) => updateDisplay({ labels })} />
          <MapToggle label="High-contrast tracks" checked={display.highContrast} onChange={(highContrast) => updateDisplay({ highContrast })} />
          <MapToggle label="Evidence lines" checked={display.evidence} onChange={(evidence) => updateDisplay({ evidence })} />
          <MapToggle label="Sensor locations" checked={display.sensors} onChange={(sensors) => updateDisplay({ sensors })} />
          <MapToggle label="Selected uncertainty" checked={display.uncertainty} onChange={(uncertainty) => updateDisplay({ uncertainty })} />
          <MapToggle label="Focus selected track" checked={display.focus} onChange={(focus) => updateDisplay({ focus })} />
        </div>
      )}
    </div>
  )
}

function MapToggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (checked: boolean) => void }) {
  return (
    <div className="track-map__row">
      <Label size="sm">{label}</Label>
      <Toggle size="sm" aria-label={label} value={checked} onChange={onChange} />
    </div>
  )
}
