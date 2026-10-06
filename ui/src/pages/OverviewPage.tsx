import { lazy, Suspense } from 'react'
import type { ServerStatus } from '../api/client'
import { StatusPanel } from './StatusPanel'

// The chart library loads only with the Overview's metrics.
const MetricsPanels = lazy(() => import('./MetricsPanels'))

export function OverviewPage({ onStatus }: { onStatus: (s: ServerStatus | null) => void }) {
  return (
    <div className="panels">
      <StatusPanel onStatus={onStatus} />
      <Suspense fallback={<span className="muted">LOADING…</span>}>
        <MetricsPanels />
      </Suspense>
    </div>
  )
}
