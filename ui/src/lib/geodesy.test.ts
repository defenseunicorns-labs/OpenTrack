import { describe, expect, it } from 'vitest'
import { EARTH_RADIUS_M, bearingLine, bearingWedge, destination, wrapLon } from './geodesy'

const dms = (d: number, m: number, s: number) => Math.sign(d || 1) * (Math.abs(d) + m / 60 + s / 3600)

describe('destination', () => {
  it('matches the published worked example (Chris Veness, movable-type.co.uk)', () => {
    // 53°19′14″N 001°43′47″W, 096°01′18″ for 124.8 km → 53°11′18″N 000°08′00″E.
    const [lat, lon] = destination(dms(53, 19, 14), -dms(1, 43, 47), dms(96, 1, 18), 124_800)
    expect(lat).toBeCloseTo(dms(53, 11, 18), 3)
    expect(lon).toBeCloseTo(dms(0, 8, 0), 3)
  })

  it('goes one degree of arc north and east on the equator', () => {
    const oneDeg = (EARTH_RADIUS_M * Math.PI) / 180
    const n = destination(0, 0, 0, oneDeg)
    expect(n[0]).toBeCloseTo(1, 9)
    expect(n[1]).toBeCloseTo(0, 9)
    const e = destination(0, 10, 90, oneDeg)
    expect(e[0]).toBeCloseTo(0, 9)
    expect(e[1]).toBeCloseTo(11, 9)
  })

  it('a great circle east from 60°N curves south', () => {
    const [lat, lon] = destination(60, 0, 90, 1_000_000)
    // Reference by rotating the position vector (independent of the formula): 58.80166°N, 17.56392°E.
    expect(lat).toBeCloseTo(58.80166, 4)
    expect(lon).toBeCloseTo(17.56392, 4)
  })

  it('wraps the antimeridian', () => {
    const [, lon] = destination(0, 179.5, 90, 111_195)
    expect(lon).toBeCloseTo(-179.5, 3)
    expect(wrapLon(190)).toBe(-170)
    expect(wrapLon(-181)).toBe(179)
  })
})

describe('bearingLine', () => {
  it('runs from the sensor to its range', () => {
    const c = bearingLine(50.6, -1.9, 47.5, 60_000)
    expect(c[0]).toEqual([-1.9, 50.6])
    const [lat, lon] = destination(50.6, -1.9, 47.5, 60_000)
    expect(c[c.length - 1][0]).toBeCloseTo(lon, 9)
    expect(c[c.length - 1][1]).toBeCloseTo(lat, 9)
    expect(c).toHaveLength(33)
  })

  it('keeps longitudes continuous across the antimeridian', () => {
    const c = bearingLine(0, 179, 90, 250_000)
    for (let i = 1; i < c.length; i++) expect(Math.abs(c[i][0] - c[i - 1][0])).toBeLessThan(1)
    expect(c[c.length - 1][0]).toBeGreaterThan(180)
  })
})

describe('bearingWedge', () => {
  it('starts and ends at the sensor and reaches both edges at range', () => {
    const w = bearingWedge(50, 0, 90, 3, 100_000)
    expect(w[0]).toEqual([0, 50])
    expect(w[w.length - 1]).toEqual([0, 50])
    const [la, lo] = destination(50, 0, 87, 100_000)
    expect(w).toContainEqual([lo, la])
  })
})
