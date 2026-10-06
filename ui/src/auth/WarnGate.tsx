import { useEffect, useState, type ReactNode } from 'react'
import { motion } from 'framer-motion'
import { Button } from '@kineticlogic/staresdk'
import { api, type WarningBanner } from '../api/client'
import { useAuth } from './context'

/**
 * The warning users accept after signing in (Settings → Banners), as OpenStare's: AGREE goes on,
 * DECLINE signs out. The server keeps the acceptance, per sign-in, and refuses the API until it has
 * it (AC-8), so this screen only asks and records.
 */
export function WarnGate({ children, banner }: { children: ReactNode; banner?: ReactNode }) {
  const { user, logout, authOn, setUser } = useAuth()
  const [warning, setWarning] = useState<WarningBanner | null | undefined>(undefined)
  const [leaving, setLeaving] = useState(false)
  const [agreeing, setAgreeing] = useState(false)
  const required = authOn && !!user?.consent_required

  useEffect(() => {
    if (!required) return
    api.warningBanner().then(setWarning, () => setWarning(null))
  }, [required])

  if (!user || !required) return <>{children}</>
  if (warning === undefined) return null
  if (!warning || !warning.enabled || !warning.text.trim()) return <>{children}</>

  const agree = async () => {
    setAgreeing(true)
    try {
      await api.acceptConsent()
      setUser({ ...user, consent_required: false })
    } finally {
      setAgreeing(false)
    }
  }
  const decline = async () => {
    setLeaving(true)
    await logout()
  }

  return (
    <>
    {banner}
    <div className="auth-screen">
      <div className="auth-column wide">
        <div className="auth-wordmark">OpenTrack</div>
        <div className="auth-subtitle">WARNING — ACKNOWLEDGMENT REQUIRED</div>
        <motion.div className="auth-card warn" initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.4, ease: 'easeOut' }}>
          <div className="warn-text">{warning.text}</div>
          <div className="warn-actions">
            <Button type="button" variant="primary" size="lg" autoFocus onClick={agree} disabled={agreeing} aria-busy={agreeing} style={{ flex: 1 }}>
              AGREE
            </Button>
            <Button type="button" variant="secondary" size="lg" onClick={decline} disabled={leaving} aria-busy={leaving} style={{ flex: 1 }}>
              {leaving ? 'SIGNING OUT…' : 'DECLINE'}
            </Button>
          </div>
        </motion.div>
      </div>
    </div>
    </>
  )
}
