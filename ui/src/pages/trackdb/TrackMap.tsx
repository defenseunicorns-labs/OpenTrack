import { useEffect, useEffectEvent, useMemo, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import { TbAdjustmentsHorizontal, TbClockOff, TbFilterOff, TbFocus2, TbLabel, TbMap, TbMapOff, TbStack2, TbZoomScan } from 'react-icons/tb'
import { AttributionControl, Map as MapLibreMap, Popup, type ExpressionSpecification, type GeoJSONSource, type LngLatBoundsLike } from 'maplibre-gl'
import ms from 'milsymbol'
import { Button, ButtonPalette, Label, SegmentedControl, Select, Slider, themeFor, Toggle, Tooltip, useTheme } from '@kineticlogic/staresdk'
import type { TrackMessage } from '../../api/client'
import { EVIDENCE_COLOR, UNCERTAINTY_COLOR } from '../../lib/palette'
import { drawableSidc, symbolUrl } from '../../lib/symbol'
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
const MIL_COLOR_MODES = ['Light', 'Medium', 'Dark'] as const
const MIL_SIZE = { min: 12, max: 64 }
const FRIENDLY_INFANTRY = symbolUrl({ standard: '2525d', code: '10031000001211000000' }, 12)
type MilColorMode = (typeof MIL_COLOR_MODES)[number]

interface MilDisplay {
  size: number
  showDirection: boolean
  fill: boolean
  colorMode: MilColorMode
}

interface MapDisplay {
  basemap: boolean
  dim: number
  trackSymbols: 'contrast' | 'mil2525'
  mil: MilDisplay
  markerSize: MarkerSize
  evidence: boolean
  sensors: boolean
  uncertainty: boolean
  focus: boolean
  labels: boolean
  /** Lost tracks on the map; off, only the selected one stays. */
  lost: boolean
}

/** A track on the map; `text` is its label on the map (the popup shows `label`); `lost`, its state is lost. */
export type TrackMapPoint = MapPoint & {
  text?: string
  lost?: boolean
  sidc: TrackMessage['sidc']
  course?: number | null
}

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
  fitTo?: MapFitTo
  onZoomToSelected?: () => void
  onClearFilters?: () => void
  paletteActions?: ReactNode
  height?: number | string
  'aria-label': string
  style?: CSSProperties
}

/** The label font: its glyph ranges are vendored by scripts/vendor-map-glyphs.sh. */
const LABEL_FONT = 'Noto Sans Medium'
const STORAGE_KEY = 'ot.map.track.display'
const CAMERA_KEY = 'ot.map.track.camera'
const DEFAULT_CENTER: [number, number] = [0, 20]
const DEFAULT_ZOOM = 0.6

interface TrackCamera {
  center: [number, number]
  zoom: number
}

function loadCamera(): TrackCamera | null {
  try {
    const saved = JSON.parse(localStorage.getItem(CAMERA_KEY) ?? 'null') as Partial<TrackCamera> | null
    const center = saved?.center
    const zoom = saved?.zoom
    if (!Array.isArray(center) || center.length !== 2 || zoom === undefined) return null
    const [lng, lat] = center
    if (!Number.isFinite(lng) || Math.abs(lng) > 180 || !Number.isFinite(lat) || Math.abs(lat) > 90) return null
    if (!Number.isFinite(zoom) || zoom < 0 || zoom > 22) return null
    return { center, zoom }
  } catch {
    return null
  }
}

const DEFAULT_DISPLAY: MapDisplay = {
  basemap: true,
  dim: 25,
  trackSymbols: 'contrast',
  mil: { size: 24, showDirection: false, fill: true, colorMode: 'Light' },
  markerSize: 'standard',
  evidence: true,
  sensors: true,
  uncertainty: true,
  focus: true,
  labels: true,
  lost: true,
}
const EMPTY = { type: 'FeatureCollection' as const, features: [] }

function loadDisplay(): MapDisplay {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}') as Partial<MapDisplay>
    return {
      ...DEFAULT_DISPLAY,
      ...saved,
      trackSymbols: saved.trackSymbols === 'mil2525' ? 'mil2525' : 'contrast',
      mil: {
        size: Number.isFinite(saved.mil?.size) ? Math.max(MIL_SIZE.min, Math.min(MIL_SIZE.max, saved.mil!.size)) : DEFAULT_DISPLAY.mil.size,
        showDirection: saved.mil?.showDirection === true,
        fill: saved.mil?.fill !== false,
        colorMode: saved.mil?.colorMode === 'Medium' || saved.mil?.colorMode === 'Dark' ? saved.mil.colorMode : 'Light',
      },
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
const MARKER_LAYERS = ['points', 'mil-symbols']
const HAS_MIL_IMAGE: ['has', string] = ['has', 'milImage']

interface MilIcon {
  symbol: InstanceType<typeof ms.Symbol>
  offset: [number, number]
}

function makeMilIcon(code: string, display: MilDisplay, direction: number | undefined): MilIcon | null {
  try {
    const symbol = new ms.Symbol(code, { size: display.size, fill: display.fill, colorMode: display.colorMode, direction })
    if (!symbol.isValid()) return null
    const { x, y } = symbol.getAnchor()
    return { symbol, offset: [-x, -y] }
  } catch {
    return null
  }
}

/** Frame tracks in the part of the map that the open drawers leave visible. */
function fitVisibleArea(map: MapLibreMap, bounds: LngLatBoundsLike, padding: number, maxZoom: number) {
  const workspace = map.getContainer().closest('.track-workspace-map')
  const cardWidth = workspace?.querySelector<HTMLElement>('.drawer-inline[aria-hidden="false"]')?.offsetWidth ?? 0
  const trayHeight = workspace?.querySelector<HTMLElement>('.track-tray')?.offsetHeight ?? 0
  const { clientWidth: width, clientHeight: height } = map.getContainer()
  if (!width || !height) return
  const side = Math.min(padding, width / 8, height / 8)
  map.fitBounds(bounds, {
    padding: {
      top: side,
      left: side,
      right: Math.min(side + cardWidth, width - 2 * side),
      bottom: Math.min(side + trayHeight, height - 2 * side),
    },
    maxZoom,
    duration: 600,
  })
}

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
  fitTo,
  onZoomToSelected,
  onClearFilters,
  paletteActions,
  height = 240,
  'aria-label': ariaLabel,
  style,
}: TrackMapProps) {
  const { theme } = useTheme()
  const palette = themeFor(theme)
  const host = useRef<HTMLDivElement>(null)
  const map = useRef<MapLibreMap | null>(null)
  const cameraPlaced = useRef(false)
  const fittedTo = useRef<string | number | null>(null)
  const milIcons = useRef(new Map<string, MilIcon | null>())
  const paletteRef = useRef(palette)
  const [ready, setReady] = useState<MapLibreMap | null>(null)
  const [display, setDisplay] = useState(loadDisplay)
  const [menu, setMenu] = useState<'display' | 'evidence' | 'mil' | null>(null)
  const [paletteTip, setPaletteTip] = useState<{ x: number; y: number; content: string } | null>(null)
  const select = useEffectEvent((id: string | null) => onSelect?.(id))
  const visiblePoints = useMemo(
    () => (display.lost ? points : points.filter((p) => !p.lost || p.id === selectedId)),
    [display.lost, points, selectedId],
  )
  const visibleBounds = useMemo(() => pointBounds(visiblePoints), [visiblePoints])
  const milMode = display.trackSymbols === 'mil2525'

  const showPaletteTip = (target: EventTarget | null) => {
    const button = target instanceof Element ? target.closest('button') : null
    const content = button?.dataset.paletteTip ?? button?.getAttribute('aria-label')
    if (!button || !content) {
      setPaletteTip(null)
      return
    }
    const { right, top, height } = button.getBoundingClientRect()
    setPaletteTip({ x: right - 8, y: top + height / 2 + 5, content })
  }

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

  const updateMil = (next: Partial<MilDisplay>) => updateDisplay({ mil: { ...display.mil, ...next } })

  useEffect(() => {
    if (!host.current) return
    const saved = loadCamera()
    cameraPlaced.current = saved !== null
    const m = new MapLibreMap({
      container: host.current,
      // Labels are drawn from glyph ranges this server serves (ui/public/map-fonts).
      style: { version: 8, glyphs: `${window.location.origin}/map-fonts/{fontstack}/{range}.pbf`, sources: {}, layers: [] },
      center: saved?.center ?? DEFAULT_CENTER,
      zoom: saved?.zoom ?? DEFAULT_ZOOM,
      attributionControl: false,
      renderWorldCopies: false,
      dragRotate: false,
      pitchWithRotate: false,
    })
    m.touchZoomRotate.disableRotation()
    m.setMissingStyleImageResolver((id) => {
      const icon = milIcons.current.get(id)
      if (!icon || m.hasImage(id)) return
      // Rasterize only the symbols MapLibre requests, at twice their display resolution.
      const canvas = icon.symbol.asCanvas(2)
      const pixels = canvas.getContext('2d')?.getImageData(0, 0, canvas.width, canvas.height)
      if (pixels) m.addImage(id, pixels, { pixelRatio: 2 })
    })
    let lastCenter = m.getCenter()
    let lastZoom = m.getZoom()
    m.on('moveend', () => {
      const center = m.getCenter()
      const zoom = m.getZoom()
      // Resizing the map can emit moveend without a camera change.
      if (center.lng === lastCenter.lng && center.lat === lastCenter.lat && zoom === lastZoom) return
      lastCenter = center
      lastZoom = zoom
      cameraPlaced.current = true
      try {
        const { lng, lat } = center.wrap()
        localStorage.setItem(CAMERA_KEY, JSON.stringify({ center: [lng, lat], zoom } satisfies TrackCamera))
      } catch {
        /* The map works even when browser storage is unavailable. */
      }
    })
    m.on('load', () => {
      if (map.current !== m) return
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
        filter: ['!', HAS_MIL_IMAGE],
        paint: {
          'circle-color': ['get', 'color'],
          'circle-radius': ['case', ['get', 'selected'], 7, 6],
          'circle-stroke-width': 2,
          'circle-stroke-color': paletteRef.current.colorBgPrimary,
        },
      })
      m.addLayer({
        id: 'mil-symbols',
        type: 'symbol',
        source: 'points',
        filter: HAS_MIL_IMAGE,
        layout: {
          'icon-image': ['get', 'milImage'],
          'icon-anchor': 'top-left',
          'icon-offset': ['get', 'milOffset'],
          'icon-allow-overlap': true,
          'icon-ignore-placement': true,
          'icon-rotation-alignment': 'map',
          'symbol-sort-key': ['case', ['get', 'selected'], 1, 0],
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
      setReady(m)
    })
    m.on('click', (event) => {
      setMenu(null)
      const hit = m.queryRenderedFeatures(event.point, { layers: MARKER_LAYERS })[0]
      select(hit ? String(hit.properties?.id) : null)
    })
    const popup = new Popup({ closeButton: false, closeOnClick: false, offset: 10, className: 'ui-map-view__popup' })
    m.on('mousemove', MARKER_LAYERS, (event) => {
      const feature = event.features?.[0]
      if (!feature) return
      m.getCanvas().style.cursor = 'pointer'
      popup.setLngLat(event.lngLat).setText(String(feature.properties?.label ?? '')).addTo(m)
    })
    m.on('mouseleave', MARKER_LAYERS, () => {
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
    if (!m || m !== ready) return
    const b = palette.offlineBasemap
    m.setPaintProperty('background', 'background-color', b.ocean)
    m.setPaintProperty('land', 'fill-color', b.landFill)
    m.setPaintProperty('land', 'fill-opacity', b.landFillOpacity)
    m.setPaintProperty('borders', 'line-color', b.countryBorder)
    // Outlines and the selection halo follow the theme: dark casing on dark, light on light.
    for (const id of ['points', 'sensors']) m.setPaintProperty(id, 'circle-stroke-color', palette.colorBgPrimary)
    m.setPaintProperty('selected-halo', 'circle-stroke-color', ['case', HAS_MIL_IMAGE, palette.colorTextPrimary, palette.colorBgPrimary])
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
    if (!m || m !== ready) return
    ;(m.getSource('outlines') as GeoJSONSource).setData(outlines ?? EMPTY)
  }, [ready, outlines])

  const tileUrl = tiles?.url.trim() || ''
  const tileAttribution = tiles?.attribution?.trim() || ''
  const tileSize = tiles?.tileSize ?? 256
  const tileMaxZoom = tiles?.maxZoom ?? 19
  useEffect(() => {
    const m = map.current
    if (!m || m !== ready || !tileUrl) return
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
    if (!m || m !== ready) return
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
    if (!m || m !== ready) return
    const radius = pointRadius(display.markerSize)
    m.setPaintProperty('points', 'circle-radius', ['case', ['get', 'selected'], radius + 2, radius])
    m.setPaintProperty('points', 'circle-stroke-width', 2.5)
    const opacity: number | ExpressionSpecification = display.focus && selectedId ? ['case', ['get', 'selected'], 1, 0.35] : 1
    m.setPaintProperty('points', 'circle-opacity', opacity)
    m.setPaintProperty('mil-symbols', 'icon-opacity', opacity)
    m.setPaintProperty('selected-halo', 'circle-radius', ['case', HAS_MIL_IMAGE, display.mil.size * 0.85 + 4, radius + 5])
    m.setPaintProperty('selected-halo', 'circle-opacity', ['case', HAS_MIL_IMAGE, 0, 1])
    m.setLayoutProperty('sensors', 'visibility', display.sensors ? 'visible' : 'none')
    for (const id of ['labels', 'labels-selected']) {
      m.setLayoutProperty(id, 'visibility', display.labels ? 'visible' : 'none')
      m.setLayoutProperty(id, 'text-radial-offset', ['case', HAS_MIL_IMAGE, display.mil.size / 11 + 0.4, 0.9])
    }
    m.setPaintProperty('labels', 'text-opacity', display.focus && selectedId ? 0.35 : 1)
    for (const source of ['lines', 'evidence', 'uncertainty']) {
      const visible = source === 'evidence' ? display.evidence : source === 'uncertainty' ? display.uncertainty : true
      m.setLayoutProperty(source, 'visibility', visible ? 'visible' : 'none')
      m.setLayoutProperty(`${source}-dashed`, 'visibility', visible ? 'visible' : 'none')
      m.setLayoutProperty(`${source}-case`, 'visibility', visible ? 'visible' : 'none')
      m.setLayoutProperty(`${source}-dashed-case`, 'visibility', visible ? 'visible' : 'none')
    }
  }, [ready, display, selectedId])

  useEffect(() => {
    const m = map.current
    if (!m || m !== ready) return
    const pointsById = new Map(visiblePoints.map((p) => [p.id, p]))
    const data = pointsToGeoJSON(visiblePoints, selectedId, palette.statusInfo)
    const previousIcons = milIcons.current
    const nextIcons = new Map<string, MilIcon | null>()
    for (const feature of data.features) {
      const point = pointsById.get(feature.properties.id)
      const properties = { ...feature.properties, text: point?.text ?? '' }
      feature.properties = properties
      const code = milMode && point ? drawableSidc(point.sidc) : null
      if (!code) continue
      const course = point?.course
      const direction = display.mil.showDirection && typeof course === 'number' && Number.isFinite(course) ? course : undefined
      const id = `mil:${code}:${display.mil.size}:${display.mil.colorMode}:${display.mil.fill}:${direction ?? ''}`
      const icon = nextIcons.has(id) ? nextIcons.get(id) : previousIcons.get(id)
      const entry = icon === undefined ? makeMilIcon(code, display.mil, direction) : icon
      nextIcons.set(id, entry)
      if (entry) {
        const symbolProperties = { ...properties, milImage: id, milOffset: entry.offset }
        feature.properties = symbolProperties
      }
    }
    milIcons.current = nextIcons
    ;(m.getSource('points') as GeoJSONSource).setData(data)
    for (const id of previousIcons.keys()) {
      if (!nextIcons.has(id) && m.hasImage(id)) m.removeImage(id)
    }
    ;(m.getSource('sensors') as GeoJSONSource).setData(pointsToGeoJSON(sensorPoints, null, EVIDENCE_COLOR))
    if (!cameraPlaced.current && visibleBounds) {
      cameraPlaced.current = true
      m.fitBounds(visibleBounds, { padding: 24, maxZoom: 9, duration: 0 })
    }
  }, [ready, visiblePoints, visibleBounds, sensorPoints, selectedId, palette.statusInfo, milMode, display.mil])

  useEffect(() => {
    const m = map.current
    if (!m || m !== ready) return
    ;(m.getSource('lines') as GeoJSONSource).setData(linesToGeoJSON(lines, palette.statusInfo))
    ;(m.getSource('evidence') as GeoJSONSource).setData(linesToGeoJSON(evidenceLines, EVIDENCE_COLOR))
    ;(m.getSource('uncertainty') as GeoJSONSource).setData(linesToGeoJSON(uncertaintyLines, UNCERTAINTY_COLOR))
  }, [ready, lines, evidenceLines, uncertaintyLines, palette.statusInfo])

  useEffect(() => {
    const m = map.current
    if (!m || m !== ready || !fitTo || fittedTo.current === fitTo.key) return
    const bounds = coordinateBounds(fitTo.coordinates)
    if (!bounds) return
    fittedTo.current = fitTo.key
    fitVisibleArea(m, bounds, fitTo.padding ?? 32, fitTo.maxZoom ?? 12)
  }, [ready, fitTo])

  return (
    <div className="track-map" style={{ height, ...style }} onKeyDown={(event) => {
      if (event.key === 'Escape') {
        setMenu(null)
        setPaletteTip(null)
      }
    }}>
      <div ref={host} role="region" aria-label={ariaLabel} className="ui-map-view track-map__canvas" />
      {/* Map buttons on a palette (its glass surface keeps them legible over the map), as OpenStare's map controls. */}
      <div
        className="track-map__controls"
        onPointerOver={(event) => { if (event.pointerType !== 'touch') showPaletteTip(event.target) }}
        onPointerLeave={() => setPaletteTip(null)}
        onFocus={(event) => showPaletteTip(event.target)}
        onBlur={() => setPaletteTip(null)}
        onScroll={() => setPaletteTip(null)}
        onClick={() => setPaletteTip(null)}
      >
        <ButtonPalette
          orientation="vertical"
          ariaLabel="Track management controls"
          gap={4}
          style={{ display: 'grid', gridTemplateColumns: 'repeat(var(--track-palette-columns), 28px)' }}
        >
          {paletteActions}
          <Button
            size="xs"
            variant="ghost"
            icon={<TbZoomScan />}
            aria-label="Fit visible tracks"
            disabled={!ready || !visibleBounds}
            onClick={() => {
              if (map.current && visibleBounds) fitVisibleArea(map.current, visibleBounds, 32, 9)
            }}
          />
          <Button
            size="xs"
            variant="ghost"
            icon={<TbFocus2 />}
            aria-label="Zoom to selected track"
            disabled={!ready || !onZoomToSelected}
            onClick={onZoomToSelected}
          />
          <Button
            size="xs"
            variant="ghost"
            icon={<TbLabel />}
            aria-label="Track labels"
            data-palette-tip={display.labels ? 'Hide track labels' : 'Show track labels'}
            aria-pressed={display.labels}
            active={display.labels}
            onClick={() => updateDisplay({ labels: !display.labels })}
          />
          <Button
            size="xs"
            variant="ghost"
            icon={<TbClockOff />}
            aria-label="Show lost tracks"
            data-palette-tip={display.lost ? 'Hide lost tracks' : 'Show lost tracks'}
            aria-pressed={display.lost}
            active={display.lost}
            onClick={() => updateDisplay({ lost: !display.lost })}
          />
          <Button
            size="xs"
            variant="ghost"
            icon={<TbStack2 />}
            aria-label="Evidence layers"
            aria-expanded={menu === 'evidence'}
            active={menu === 'evidence'}
            onClick={() => setMenu((open) => open === 'evidence' ? null : 'evidence')}
          />
          <Button
            size="xs"
            variant="ghost"
            icon={display.basemap ? <TbMapOff /> : <TbMap />}
            aria-label={display.basemap ? 'Hide basemap' : 'Show basemap'}
            onClick={() => updateDisplay({ basemap: !display.basemap })}
          />
          <Button
            size="xs"
            variant="ghost"
            icon={<TbAdjustmentsHorizontal />}
            aria-label="Map display options"
            aria-expanded={menu === 'display'}
            active={menu === 'display'}
            onClick={() => setMenu((open) => open === 'display' ? null : 'display')}
          />
          {milMode && (
            <Button
              size="xs"
              variant="ghost"
              icon={FRIENDLY_INFANTRY && <img src={FRIENDLY_INFANTRY} width={20} height={20} alt="" />}
              aria-label="MIL-2525 symbol settings"
              aria-expanded={menu === 'mil'}
              active={menu === 'mil'}
              onClick={() => setMenu((open) => open === 'mil' ? null : 'mil')}
            />
          )}
          {onClearFilters && (
            <Button size="xs" variant="ghost" icon={<TbFilterOff />} aria-label="Clear filters" data-palette-tip="Clear search and filters" onClick={onClearFilters} />
          )}
        </ButtonPalette>
      </div>
      {paletteTip && <Tooltip {...paletteTip} />}
      {menu === 'display' && (
        <div className="track-map__menu" role="dialog" aria-label="Map display options">
          <h4 className="subhead">Map display</h4>
          <div className="track-map__row track-map__row--stacked">
            <Label size="sm">Basemap dimming</Label>
            <Slider aria-label="Basemap dimming" min={0} max={60} step={5} value={display.dim} onChange={(dim) => updateDisplay({ dim })} formatValue={(v) => `${v}%`} />
          </div>
          <div className="track-map__row track-map__row--stacked">
            <Label size="sm">Track symbols</Label>
            <SegmentedControl
              size="sm"
              aria-label="Track symbols"
              value={display.trackSymbols}
              onChange={(trackSymbols) => updateDisplay({ trackSymbols })}
              options={[
                { value: 'contrast', label: 'High Contrast' },
                { value: 'mil2525', label: 'MIL-2525' },
              ]}
            />
          </div>
          {!milMode && (
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
          )}
          <MapToggle label="Focus selected track" checked={display.focus} onChange={(focus) => updateDisplay({ focus })} />
        </div>
      )}
      {menu === 'evidence' && (
        <div className="track-map__menu" role="dialog" aria-label="Evidence layers">
          <h4 className="subhead">Evidence layers</h4>
          <MapToggle label="Evidence lines" checked={display.evidence} onChange={(evidence) => updateDisplay({ evidence })} />
          <MapToggle label="Sensor locations" checked={display.sensors} onChange={(sensors) => updateDisplay({ sensors })} />
          <MapToggle label="Selected uncertainty" checked={display.uncertainty} onChange={(uncertainty) => updateDisplay({ uncertainty })} />
        </div>
      )}
      {milMode && menu === 'mil' && (
        <div className="track-map__menu" role="dialog" aria-label="MIL-2525 symbol settings">
          <h4 className="subhead">MIL-2525 symbols</h4>
          <div className="track-map__row track-map__row--stacked">
            <Label size="sm">Symbol size</Label>
            <Slider aria-label="Symbol size" min={MIL_SIZE.min} max={MIL_SIZE.max} step={2} value={display.mil.size} onChange={(size) => updateMil({ size })} />
          </div>
          <MapToggle label="Directional leader lines" checked={display.mil.showDirection} onChange={(showDirection) => updateMil({ showDirection })} />
          <MapToggle label="Fill" checked={display.mil.fill} onChange={(fill) => updateMil({ fill })} />
          <div className="track-map__row track-map__row--stacked">
            <Label size="sm">Color mode</Label>
            <SegmentedControl
              size="sm"
              aria-label="Color mode"
              value={display.mil.colorMode}
              onChange={(colorMode) => updateMil({ colorMode })}
              options={MIL_COLOR_MODES.map((mode) => ({ value: mode, label: mode }))}
            />
          </div>
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
