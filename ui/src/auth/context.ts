import { createContext, useContext } from 'react'
import type { AuthPublic, Me, Role } from '../api/client'

const ORDER: Role[] = ['viewer', 'track_manager', 'admin']

/** Whether `role` is at least `min` (viewer < track manager < admin). */
export const roleAtLeast = (role: Role | undefined, min: Role) => role != null && ORDER.indexOf(role) >= ORDER.indexOf(min)

/** Whether `user` can do what needs `min` here: the role, and for admin work an address that serves the admin routes. */
export const canHere = (user: Pick<Me, 'role' | 'admin_api'> | null | undefined, min: Role) =>
  roleAtLeast(user?.role, min) && !(min === 'admin' && user?.admin_api === false)

export const ROLE_LABEL: Record<Role, string> = { viewer: 'Viewer', track_manager: 'Track manager', admin: 'Admin' }

export interface AuthState {
  /** The signed-in account (with sign-in off, the server's stand-in admin). */
  user: Me | null
  /** What the sign-in page offers; null until loaded or if the server is unreachable. */
  config: AuthPublic | null
  /** Whether sign-in is on (false with OT_AUTH=off). */
  authOn: boolean
  loading: boolean
  hasRole: (min: Role) => boolean
  login: (email: string, password: string) => Promise<void>
  /** Sign out and go to the sign-in page. */
  logout: () => Promise<void>
  setUser: (me: Me) => void
}

export const AuthCtx = createContext<AuthState | null>(null)

export function useAuth(): AuthState {
  const v = useContext(AuthCtx)
  if (!v) throw new Error('useAuth outside AuthProvider')
  return v
}

/** Whether the signed-in account has at least `min` (what the server's policy needs for a change). */
export function useCan(min: Role): boolean {
  return useAuth().hasRole(min)
}

/** A return path that stays on this site (no scheme, no protocol-relative URL, never /login). */
export function safeReturnPath(raw: string | null): string {
  if (!raw || !raw.startsWith('/') || raw.startsWith('//') || raw.startsWith('/\\') || raw.includes(':')) return '/'
  if (raw === '/login' || raw.startsWith('/login?') || raw.startsWith('/login#')) return '/'
  return raw
}

/** Where this page is, as a `from` value. */
export const here = () => window.location.pathname + window.location.search + window.location.hash
