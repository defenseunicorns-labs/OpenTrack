import { useEffect } from 'react'
import { useToast } from '@kineticlogic/staresdk'
import { fmtTime } from '../lib/format'
import { useAuth } from './context'

const WEEK = 7 * 86_400_000

/**
 * Once per session, after signing in: when the account last signed in and how many sign-ins
 * failed since (a warning when any did), and a password that expires within a week.
 */
export function LastLoginNotice() {
  const { user } = useAuth()
  const { toast } = useToast()
  useEffect(() => {
    if (!user?.session || !user.last_login) return
    const key = `ot.lastlogin.${user.session}`
    try {
      if (sessionStorage.getItem(key)) return
      sessionStorage.setItem(key, '1')
    } catch {
      /* storage unavailable: shown on every load */
    }
    const { previous_at_ms: prev, failed_attempts: failed } = user.last_login
    const since = failed === 1 ? '1 failed sign-in since.' : `${failed} failed sign-ins since.`
    toast({
      variant: failed > 0 ? 'warning' : 'info',
      title: 'Signed in',
      message: (prev ? `Your last sign-in was ${fmtTime(prev)}. ` : 'This is your first sign-in. ') + since,
    })
    const exp = user.password_expires_at_ms
    if (exp != null && exp - Date.now() < WEEK)
      toast({ variant: 'warning', title: 'Password', message: `Your password expires ${fmtTime(exp)}. Change it from your account menu.` })
  }, [user, toast])
  return null
}
