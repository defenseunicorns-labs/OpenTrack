import { useState } from 'react'
import { motion } from 'framer-motion'
import { Button, Input } from '@kineticlogic/staresdk'
import { api, describePolicy } from '../api/client'
import { errorMessage } from '../lib/format'
import { useAuth } from './context'

/**
 * Shown instead of the app while the account's password must change: a temporary one an admin
 * set, or one past its maximum age. Nothing else can be done until it changes (the server refuses
 * every other call), except signing out.
 */
export function PasswordChangeScreen() {
  const { user, setUser, logout } = useAuth()
  const [current, setCurrent] = useState('')
  const [next, setNext] = useState('')
  const [again, setAgain] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [expired] = useState(() => user?.password_expires_at_ms != null && user.password_expires_at_ms < Date.now())

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    if (next !== again) return setError('The new passwords differ.')
    setBusy(true)
    setError(null)
    try {
      setUser(await api.changePassword(current, next))
    } catch (err) {
      setError(errorMessage(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="auth-screen">
      <div className="auth-column">
        <div className="auth-wordmark">OpenTrack</div>
        <div className="auth-subtitle">{expired ? 'PASSWORD EXPIRED' : 'NEW PASSWORD REQUIRED'}</div>
        <motion.form className="auth-card" onSubmit={submit} initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.4, ease: 'easeOut' }}>
          <div className="muted">
            {expired ? 'Your password has expired.' : 'Your password was set by an admin and is temporary.'} Choose a new one to go on.{' '}
            {describePolicy(user?.password_policy)}
          </div>
          <div className="auth-field">
            <label htmlFor="fpw-current">CURRENT PASSWORD</label>
            <Input id="fpw-current" size="lg" type="password" autoComplete="current-password" autoFocus required value={current} onChange={(e) => setCurrent(e.target.value)} />
          </div>
          <div className="auth-field">
            <label htmlFor="fpw-new">NEW PASSWORD</label>
            <Input id="fpw-new" size="lg" type="password" autoComplete="new-password" required value={next} onChange={(e) => setNext(e.target.value)} />
          </div>
          <div className="auth-field">
            <label htmlFor="fpw-again">NEW PASSWORD AGAIN</label>
            <Input id="fpw-again" size="lg" type="password" autoComplete="new-password" required value={again} onChange={(e) => setAgain(e.target.value)} />
          </div>
          <Button type="submit" size="xl" variant="primary" fullWidth disabled={busy || !current || !next}>
            {busy ? 'CHANGING…' : 'CHANGE PASSWORD'}
          </Button>
          <Button type="button" size="lg" variant="ghost" fullWidth onClick={() => logout()}>
            Sign out
          </Button>
          {error && (
            <div className="auth-error" role="alert">
              {error}
            </div>
          )}
        </motion.form>
      </div>
    </div>
  )
}
