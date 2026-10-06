import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useToast } from '@kineticlogic/staresdk'
import { api, ApiError, withCsrf, type AuthPublic, type Me, type Role } from '../api/client'
import { trackActivity, withIdle } from './activity'
import { AuthCtx, canHere, here } from './context'

/** Calls whose 401 is an answer (wrong password, not signed in yet), not a lapsed session. */
const QUIET_401 = ['/api/v1/auth/login', '/api/v1/auth/me', '/api/v1/auth/password', '/api/v1/auth/public']

type Wrapped = typeof window.fetch & { __otAuth?: boolean }

/**
 * Who is signed in, loaded from /auth/me at start. Any other /api call answered 401 means the
 * session ended (signed out, idle too long, ended elsewhere): go to the sign-in page, coming back
 * here after. A 403 (role too low) is shown as a toast with the server's reason. Every /api call
 * says how long the user has been idle, so polling alone does not keep a session alive, and
 * carries the header that shows it comes from this page (the server's CSRF check).
 */
export function AuthProvider({ children }: { children: ReactNode }) {
  const { toast } = useToast()
  const [user, setUser] = useState<Me | null>(null)
  const [config, setConfig] = useState<AuthPublic | null>(null)
  const [loading, setLoading] = useState(true)
  const toastRef = useRef(toast)
  useEffect(() => {
    toastRef.current = toast
  }, [toast])

  useEffect(() => trackActivity(), [])

  useEffect(() => {
    const original = window.fetch
    if ((original as Wrapped).__otAuth) return
    const wrapped: Wrapped = async (input, init) => {
      const url = typeof input === 'string' ? input : input instanceof URL ? input.pathname : input.url
      const path = url.startsWith('http') ? new URL(url).pathname : url
      if (!path.startsWith('/api/')) return original(input, init)
      const res = await original(input, withCsrf(input, withIdle(input, init)))
      if (res.status === 401 && !QUIET_401.some((p) => path.startsWith(p)) && window.location.pathname !== '/login') {
        window.location.assign('/login?ended=1&from=' + encodeURIComponent(here()))
      } else if (res.status === 403 && !path.startsWith('/api/v1/auth/login')) {
        res
          .clone()
          .json()
          .then(
            (b: { error?: string; code?: string }) => b,
            () => ({}) as { error?: string; code?: string },
          )
          .then((b) => {
            // A password to change first: the page shows the change screen.
            // A password to change or the warning to accept first: the page shows that screen.
            if (b.code === 'password_change_required' || b.code === 'consent_required') window.location.reload()
            else toastRef.current({ variant: 'error', title: 'Not allowed', message: b.error ?? 'Your role does not allow this.', dedupeKey: `403:${b.error}` })
          })
      }
      return res
    }
    wrapped.__otAuth = true
    window.fetch = wrapped
    return () => {
      window.fetch = original
    }
  }, [])

  useEffect(() => {
    let cancelled = false
    Promise.allSettled([api.authPublic(), api.me()]).then(([pub, me]) => {
      if (cancelled) return
      if (pub.status === 'fulfilled') setConfig(pub.value)
      if (me.status === 'fulfilled') setUser(me.value)
      else if (!(me.reason instanceof ApiError && me.reason.status === 401)) console.warn('auth/me', me.reason)
      setLoading(false)
    })
    return () => {
      cancelled = true
    }
  }, [])

  const login = useCallback(async (email: string, password: string) => {
    // The caller loads the page it returns to, which reads the new session.
    await api.login(email, password)
  }, [])

  const logout = useCallback(async () => {
    try {
      await api.logout()
    } catch {
      /* signed out here whatever the server says */
    } finally {
      // A full load of the sign-in page, rather than clearing `user` here: that would race the
      // guard's own redirect to /login?from=…
      window.location.assign('/login?signed_out=1')
    }
  }, [])

  const authOn = config ? config.auth : user?.via !== 'disabled'
  // Admin areas are hidden where the admin routes aren't served (OT_ADMIN_BIND).
  const hasRole = useCallback((min: Role) => canHere(user, min), [user])

  const value = useMemo(() => ({ user, config, authOn, loading, hasRole, login, logout, setUser }), [user, config, authOn, loading, hasRole, login, logout])
  return <AuthCtx.Provider value={value}>{children}</AuthCtx.Provider>
}
