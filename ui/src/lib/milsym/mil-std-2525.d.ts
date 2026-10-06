// The `mil-std-2525` package ships no types: the parts the symbol designer reads. `ms2525d` is
// keyed by 2-digit symbol set; each set's `mainIcon` rows are its symbols, and only control
// measures (set 25) carry a 'Geometric Rendering' (Point, Line or Area).
declare module 'mil-std-2525' {
  export interface Ms2525Row {
    Entity: string
    'Entity Type': string
    'Entity Subtype': string
    Code: string
    Remarks?: string
    'Geometric Rendering'?: string
  }

  export interface Ms2525SymbolSet {
    symbolset: string
    name: string
    mainIcon: Ms2525Row[]
    modifier1: unknown
    modifier2: unknown
  }

  export const ms2525d: Record<string, Ms2525SymbolSet>
}
