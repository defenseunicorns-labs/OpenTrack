import { useEffect, useRef, useState } from 'react'
import { api, type PreviewResult, type SourceSpec } from '../api/client'
import { errorMessage } from './format'
import { useCan } from '../auth/context'

const PREVIEW_DELAY_MS = 600

/**
 * A dry run of `spec` over `sampleSourceId`'s stored probe samples, re-run shortly after the spec
 * stops changing. Nothing is saved or published. `trace` > 0 also follows that many samples stage by
 * stage.
 */
export function usePreview(spec: SourceSpec, sampleSourceId: string, nonce = 0, trace = 0) {
  const [preview, setPreview] = useState<PreviewResult | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [running, setRunning] = useState(false)
  const seq = useRef(0)
  const canAdmin = useCan('admin')

  useEffect(() => {
    if (!canAdmin) return
    const id = ++seq.current
    const t = setTimeout(() => {
      setRunning(true)
      api
        .preview(spec, sampleSourceId, [], trace)
        .then((r) => {
          if (id !== seq.current) return
          setPreview(r)
          setError(null)
        })
        .catch((e) => {
          if (id !== seq.current) return
          setError(errorMessage(e))
        })
        .finally(() => id === seq.current && setRunning(false))
    }, PREVIEW_DELAY_MS)
    return () => clearTimeout(t)
  }, [spec, sampleSourceId, nonce, canAdmin, trace])

  // The dry run is an admin's (it can reach the source's own transport).
  if (!canAdmin) return { preview: null, error: 'The preview needs the admin role.', running: false }
  return { preview, error, running }
}
