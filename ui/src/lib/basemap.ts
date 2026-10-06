import { createContext, useContext } from 'react'
import type { MapTiles } from '@kineticlogic/staresdk/map-view'

/** The maps' basemap tiles, fetched through OpenTrack (Settings → General: Basemap tiles). */
export const BASEMAP_TILES: MapTiles = { url: '/api/v1/basemap/{z}/{x}/{y}', maxZoom: 19 }

/** Whether basemap tiles are on, from the settings the header already loads. */
export const BasemapCtx = createContext(false)

/** The `tiles` prop for a MapView: the proxied tiles when they are on, else none (the outlines). */
export function useBasemapTiles(): MapTiles | undefined {
  return useContext(BasemapCtx) ? BASEMAP_TILES : undefined
}
