// Great-circle geometry on a spherical Earth, for drawing on the map.

/** Mean Earth radius (IUGG), metres. */
export const EARTH_RADIUS_M = 6371008.8

/**
 * A bearing's range when its sensor does not say, metres. The same as the
 * server's default (`DEFAULT_RANGE_M` in crates/ot-server/src/engine/nonpoint.rs),
 * which omits `max_range_m` from a track's bearings when it is this value.
 */
export const DEFAULT_BEARING_RANGE_M = 250_000

const rad = (d: number) => (d * Math.PI) / 180
const deg = (r: number) => (r * 180) / Math.PI

/** Longitude wrapped to [-180, 180). */
export function wrapLon(lon: number): number {
  return ((((lon + 180) % 360) + 360) % 360) - 180
}

/**
 * The point `distanceM` metres from (`lat`, `lon`) along the great circle that
 * starts at `bearingDeg` (degrees true), as [lat, lon] in degrees, longitude in
 * [-180, 180).
 */
export function destination(lat: number, lon: number, bearingDeg: number, distanceM: number): [number, number] {
  const d = distanceM / EARTH_RADIUS_M
  const p1 = rad(lat)
  const t = rad(bearingDeg)
  const sinP2 = Math.sin(p1) * Math.cos(d) + Math.cos(p1) * Math.sin(d) * Math.cos(t)
  const p2 = Math.asin(Math.max(-1, Math.min(1, sinP2)))
  const l2 = rad(lon) + Math.atan2(Math.sin(t) * Math.sin(d) * Math.cos(p1), Math.cos(d) - Math.sin(p1) * sinP2)
  return [deg(p2), wrapLon(deg(l2))]
}

/**
 * A line of bearing as map coordinates ([lon, lat] pairs), from the sensor out
 * along the great circle to `rangeM`, in `segments` pieces so it bends as a
 * great circle does on the map. Longitudes stay continuous from the sensor's
 * (they may pass ±180) so a line across the antimeridian doesn't jump.
 */
export function bearingLine(lat: number, lon: number, bearingDeg: number, rangeM: number, segments = 32): [number, number][] {
  const out: [number, number][] = [[lon, lat]]
  let prev = lon
  for (let i = 1; i <= segments; i++) {
    const [la, lo] = destination(lat, lon, bearingDeg, (rangeM * i) / segments)
    const l = prev + wrapLon(lo - prev)
    out.push([l, la])
    prev = l
  }
  return out
}

/**
 * The outline of a bearing's ±`sigmaDeg` wedge as map coordinates: from the
 * sensor out along one edge, round the arc at `rangeM`, and back along the other.
 */
export function bearingWedge(lat: number, lon: number, bearingDeg: number, sigmaDeg: number, rangeM: number, segments = 32): [number, number][] {
  const left = bearingLine(lat, lon, bearingDeg - sigmaDeg, rangeM, segments)
  const right = bearingLine(lat, lon, bearingDeg + sigmaDeg, rangeM, segments).reverse()
  const arc: [number, number][] = []
  const steps = Math.max(2, Math.ceil(sigmaDeg))
  let prev = left[left.length - 1][0]
  for (let i = 1; i < steps; i++) {
    const [la, lo] = destination(lat, lon, bearingDeg - sigmaDeg + (2 * sigmaDeg * i) / steps, rangeM)
    const l = prev + wrapLon(lo - prev)
    arc.push([l, la])
    prev = l
  }
  return [...left, ...arc, ...right]
}
