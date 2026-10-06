import { useCallback, useEffect, useState } from 'react'
import { TbLogout } from 'react-icons/tb'
import { Badge, Button, DataTable, useToast, type DataTableColumn } from '@kineticlogic/staresdk'
import { api, type SessionRow } from '../api/client'
import { errorMessage, fmtTime } from '../lib/format'

/** A browser, shortly: its name and platform from the user agent. */
function browserName(ua: string | null): string {
  if (!ua) return '—'
  const name = /Edg\//.test(ua) ? 'Edge' : /Firefox\//.test(ua) ? 'Firefox' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : ua.split(' ')[0]
  const os = /Windows/.test(ua) ? 'Windows' : /Mac OS X/.test(ua) ? 'macOS' : /Android/.test(ua) ? 'Android' : /Linux/.test(ua) ? 'Linux' : /iPhone|iPad/.test(ua) ? 'iOS' : ''
  return os ? `${name} on ${os}` : name
}

/**
 * Live sessions with a button to end each: your own, or (`all`, admins) everyone's. The current
 * one is marked; ending it signs this browser out.
 */
export function SessionsTable({ all = false, reloadKey = 0 }: { all?: boolean; reloadKey?: number }) {
  const { toast, confirm } = useToast()
  const [rows, setRows] = useState<SessionRow[] | null>(null)
  const [current, setCurrent] = useState<string | null>(null)
  const load = useCallback(() => {
    api.sessions({ all }).then(
      (r) => {
        setRows(r.sessions)
        setCurrent(r.current)
      },
      (e) => toast({ variant: 'error', title: 'Sessions', message: errorMessage(e) }),
    )
  }, [all, toast])
  useEffect(load, [load, reloadKey])

  const end = async (r: SessionRow) => {
    const mine = r.id === current
    if (!(await confirm(mine ? 'This browser is signed out now.' : `The session of ${r.user_email} from ${r.ip ?? 'an unknown address'} ends now.`, { title: 'End session', confirmLabel: 'End' })))
      return
    try {
      await api.endSession(r.id)
      if (mine) window.location.assign('/login')
      else load()
    } catch (e) {
      toast({ variant: 'error', title: 'Not ended', message: errorMessage(e) })
    }
  }

  const columns: DataTableColumn<SessionRow>[] = [
    ...(all ? [{ key: 'user', header: 'Account', render: (r: SessionRow) => r.user_email, sortValue: (r: SessionRow) => r.user_email }] : []),
    {
      key: 'browser',
      header: 'Browser',
      render: (r) => (
        <span className="num-row" title={r.user_agent ?? undefined}>
          {browserName(r.user_agent)}
          {r.id === current && (
            <Badge size="sm" color="brand">
              this one
            </Badge>
          )}
        </span>
      ),
    },
    { key: 'ip', header: 'Address', width: 140, mono: true, render: (r) => r.ip ?? '—', sortValue: (r) => r.ip },
    { key: 'created', header: 'Signed in', width: 170, mono: true, render: (r) => fmtTime(r.created_at_ms), sortValue: (r) => r.created_at_ms },
    { key: 'seen', header: 'Last used', width: 170, mono: true, render: (r) => fmtTime(r.last_seen_ms), sortValue: (r) => r.last_seen_ms },
    { key: 'expires', header: 'Ends by', width: 170, mono: true, render: (r) => fmtTime(r.expires_at_ms), sortValue: (r) => r.expires_at_ms },
    {
      key: 'actions',
      header: '',
      align: 'right',
      width: 50,
      render: (r) => <Button size="xs" variant="ghost" icon={<TbLogout />} title="End this session" aria-label={`End the session of ${r.user_email} from ${r.ip ?? 'unknown'}`} onClick={() => end(r)} />,
    },
  ]
  if (rows === null) return <span className="muted">LOADING…</span>
  return <DataTable aria-label={all ? 'Every session' : 'Your sessions'} columns={columns} rows={rows} rowKey={(r) => r.id} density="compact" empty="No live sessions." defaultSort={{ key: 'created', direction: 'desc' }} />
}
