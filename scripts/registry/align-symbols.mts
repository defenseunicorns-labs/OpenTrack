/**
 * Align every registry entity's symbol fields (domain, affiliation, CoT type, SIDC) with the rules
 * the entity editor keeps them by (ui/src/lib/milsym/sync.ts), through the API, so each change is
 * an ordinary audited entity revision.
 *
 * Per entity: the stored symbol fills a missing code (from the SIDC if it is one, else the CoT
 * type; a 2525C SIDC is stored as its 2525D equivalent; an existing code is never replaced, so a
 * re-run changes nothing), then an explicitly set domain and affiliation win
 * and the codes follow them. An entity with no symbol but a domain gets the domain's generic one.
 * A blank domain or affiliation is left blank (it means "the feed's").
 *
 * Usage (from ui/): OT_URL=http://127.0.0.1:8090 OT_TOKEN=… npx tsx ../scripts/registry/align-symbols.mts [--apply]
 * Without --apply it only reports what it would change.
 */
import { syncSymbol, type SymbolFields } from '../../ui/src/lib/milsym/sync'

const base = (process.env.OT_URL ?? 'http://127.0.0.1:8090').replace(/\/$/, '') + '/api/v1'
const token = process.env.OT_TOKEN
if (!token) throw new Error('OT_TOKEN is not set')
const apply = process.argv.includes('--apply')
const headers = { authorization: `Bearer ${token}`, 'content-type': 'application/json', 'x-opentrack-actor': 'align-symbols' }

type Entity = Record<string, unknown> & { id: string; name?: string | null }

async function all(): Promise<Entity[]> {
  const out: Entity[] = []
  for (let offset = 0; ; offset += 500) {
    const r = await fetch(`${base}/registry/entities?q=&limit=500&offset=${offset}`, { headers })
    if (!r.ok) throw new Error(`list: ${r.status} ${await r.text()}`)
    const page = (await r.json()) as { entities: Entity[]; total: number }
    out.push(...page.entities)
    if (out.length >= page.total || page.entities.length === 0) return out
  }
}

const text = (v: unknown) => (typeof v === 'string' ? v : '')

/** The aligned fields for an entity's stored ones. */
function aligned(f: SymbolFields): SymbolFields {
  // Fill a missing code from the one there is; never replace an existing one (a CoT type can be
  // more specific than any 2525D symbol). The SIDC first, else the CoT type; a 2525C SIDC is
  // stored as its 2525D equivalent.
  const fromSidc = syncSymbol(f, 'sidc')
  const fromCot = syncSymbol(f, 'cot_type')
  const sidcOk = /^\d{20}$/.test(fromSidc.sidc)
  const cotOk = /^a-[a-z]-[A-Z]/.test(f.cot_type)
  let g = { ...f }
  if (sidcOk && cotOk) g = { ...f, sidc: fromSidc.sidc }
  else if (sidcOk) g = fromSidc
  else if (cotOk) g = fromCot
  // A blank domain or affiliation stays blank: it means "the feed's", which a value read off the
  // symbol would override for every linked track.
  g = { ...g, domain: f.domain, affiliation: f.affiliation }
  if (f.domain) g = syncSymbol(g, 'domain')
  if (f.affiliation) g = syncSymbol(g, 'affiliation')
  return g
}

const entities = await all()
let changed = 0
let failed = 0
const kinds: Record<string, number> = {}
for (const e of entities) {
  const f: SymbolFields = { domain: text(e.domain), affiliation: text(e.affiliation), cot_type: text(e.cot_type), sidc: text(e.sidc) }
  const g = aligned(f)
  const diff = (Object.keys(f) as (keyof SymbolFields)[]).filter((k) => f[k] !== g[k])
  if (!diff.length) continue
  changed++
  for (const k of diff) kinds[`${k}: ${f[k] ? 'changed' : 'filled'}`] = (kinds[`${k}: ${f[k] ? 'changed' : 'filled'}`] ?? 0) + 1
  const lossy = f.sidc && /^\d{20}$/.test(f.sidc) && f.sidc.slice(10, 16) !== '000000' && g.sidc.slice(10, 16) === '000000'
  if (changed <= 15 || lossy)
    console.log(`${lossy ? 'GENERIC ' : ''}${e.id} ${e.name ?? ''}: ${diff.map((k) => `${k} ${JSON.stringify(f[k])} -> ${JSON.stringify(g[k])}`).join('; ')}`)
  if (!apply) continue
  const body = { ...e, ...Object.fromEntries(diff.map((k) => [k, g[k] || null])) }
  const r = await fetch(`${base}/registry/entities/${encodeURIComponent(e.id)}`, { method: 'PUT', headers, body: JSON.stringify(body) })
  if (!r.ok) {
    failed++
    console.error(`FAILED ${e.id}: ${r.status} ${(await r.text()).slice(0, 200)}`)
  }
}
console.log(`${entities.length} entities; ${changed} ${apply ? 'updated' : 'would change'}${failed ? `, ${failed} failed` : ''}`)
console.log(JSON.stringify(kinds))
