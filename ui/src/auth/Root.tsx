import { lazy, Suspense, useEffect, useState } from 'react'
import { api, type Banner } from '../api/client'
import { LastLoginNotice } from './LastLoginNotice'
import { LoginPage } from './LoginPage'
import { PasswordChangeScreen } from './PasswordChangeScreen'
import { WarnGate } from './WarnGate'
import { here, safeReturnPath, useAuth } from './context'

const App = lazy(() => import('../App'))

/** The classification banner on the sign-in and warning pages (the app shows its own). */
function PublicBanner() {
  const [banner, setBanner] = useState<Banner | null>(null)
  useEffect(() => {
    api.banner().then((b) => setBanner(b.enabled ? b : null), () => {})
  }, [])
  if (!banner) return null
  return (
    <>
      <div className="classification-bar top" style={{ background: banner.background, color: banner.color }}>
        {banner.text}
      </div>
      <div className="classification-bar bottom" style={{ background: banner.background, color: banner.color }}>
        {banner.text}
      </div>
    </>
  )
}

/** Sign-in page at /login; everything else needs an account (unless sign-in is off) and the accepted warning. */
export function Root() {
  const { user, loading, authOn } = useAuth()
  const onLogin = window.location.pathname === '/login'
  const bounce = !loading && onLogin && (user != null || !authOn)
  const toLogin = !loading && !onLogin && authOn && user == null

  useEffect(() => {
    if (bounce) window.location.replace(safeReturnPath(new URLSearchParams(window.location.search).get('from')))
    else if (toLogin) window.location.replace('/login?from=' + encodeURIComponent(here()))
  }, [bounce, toLogin])

  if (loading || bounce || toLogin) return null
  if (onLogin)
    return (
      <>
        <PublicBanner />
        <LoginPage />
      </>
    )
  return (
    <>
      {!authOn && (
        <div className="auth-off-banner" role="alert">
          Authentication disabled (OT_AUTH=off): anyone who reaches this server is an admin
        </div>
      )}
      <WarnGate banner={<PublicBanner />}>
        {user?.must_change_password ? (
          <>
            <PublicBanner />
            <PasswordChangeScreen />
          </>
        ) : (
          <>
            <LastLoginNotice />
            <Suspense fallback={null}>
              <App />
            </Suspense>
          </>
        )}
      </WarnGate>
    </>
  )
}
