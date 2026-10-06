import { useEffect, useState } from 'react'
import { motion } from 'framer-motion'
import { Button, Input } from '@kineticlogic/staresdk'
import { ApiError } from '../api/client'
import { safeReturnPath, useAuth } from './context'

/** Where to go after signing in: `?from=` when it stays on this site. */
function returnPath() {
  const params = new URLSearchParams(window.location.search)
  if (params.has('from')) return safeReturnPath(params.get('from'))
  return window.location.pathname === '/login' ? '/' : safeReturnPath(window.location.pathname + window.location.hash)
}

/** The sign-in page, laid out as OpenStare's. */
export function LoginPage() {
  const { login, config } = useAuth()
  const [email, setEmail] = useState('')
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(() => {
    const q = new URLSearchParams(window.location.search)
    if (q.get('sso') === 'failed') return 'Single sign-on failed. Try again or ask an admin.'
    if (q.get('signed_out') === '1') return 'You have signed out. Your session has ended; close the browser to finish.'
    if (q.get('ended') === '1') return 'Your session ended (signed out, idle too long, or ended elsewhere). Sign in again.'
    return null
  })
  // Arriving here means signed out (or never signed in): the warning is asked again.
  useEffect(() => {
    try {
      Object.keys(sessionStorage)
        .filter((k) => k.startsWith('ot.warn.ack.'))
        .forEach((k) => sessionStorage.removeItem(k))
    } catch {
      /* storage unavailable */
    }
  }, [])
  const passwordOn = config?.password_login ?? true
  const saml = config?.saml.enabled ? config.saml : null
  const openstare = config?.openstare.enabled && config.openstare.login_url ? config.openstare : null

  const submit = async (e: React.FormEvent) => {
    e.preventDefault()
    setError(null)
    setBusy(true)
    try {
      await login(email.trim(), password)
      window.location.assign(returnPath())
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) setError('Invalid email or password, or the account is locked or turned off.')
      else if (err instanceof ApiError && err.status === 429) setError('Too many attempts. Wait a moment and try again.')
      else if (err instanceof ApiError && err.status < 500) setError(err.message)
      else setError('Service unavailable. Try again.')
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className="auth-screen">
      <div className="auth-column">
        <div className="auth-wordmark">OpenTrack</div>
        <div className="auth-subtitle">AUTHENTICATION REQUIRED</div>
        <motion.form className="auth-card" onSubmit={submit} initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} transition={{ duration: 0.4, ease: 'easeOut' }}>
          {passwordOn && (
            <>
              <div className="auth-field">
                <label htmlFor="login-email">EMAIL</label>
                <Input id="login-email" size="lg" type="email" autoComplete="username" autoFocus required value={email} onChange={(e) => setEmail(e.target.value)} />
              </div>
              <div className="auth-field">
                <label htmlFor="login-password">PASSWORD</label>
                <Input id="login-password" size="lg" type="password" autoComplete="current-password" required value={password} onChange={(e) => setPassword(e.target.value)} />
              </div>
              <Button type="submit" size="xl" variant="primary" fullWidth disabled={busy}>
                {busy ? 'AUTHENTICATING…' : 'ACCESS SYSTEM'}
              </Button>
            </>
          )}
          {saml && (
            <Button type="button" size="xl" variant="secondary" fullWidth onClick={() => window.location.assign('/api/v1/auth/saml/login')}>
              {saml.label || 'Sign in with SSO'}
            </Button>
          )}
          {openstare && (
            <Button type="button" size="xl" variant="secondary" fullWidth onClick={() => window.location.assign(openstare.login_url)}>
              Sign in with OpenStare
            </Button>
          )}
          {!passwordOn && !saml && !openstare && <div className="auth-error">No way to sign in is set up. Ask an admin.</div>}
          {error && (
            <div className="auth-error" role="alert">
              <span className="auth-error-tag">AUTH ERROR: </span>
              {error}
            </div>
          )}
        </motion.form>
      </div>
    </div>
  )
}
