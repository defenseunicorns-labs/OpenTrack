import { describe, expect, it } from 'vitest'
import { GUIDES, guideHeadings, helpHref, parseHelpPath, renderableGuide, slugify } from '../../lib/help'
import { guideMarkdown } from './guides'

/**
 * Anchors the UI's info tips (and other docs) link to. Renaming a heading must keep these: add a
 * `<a id="…"></a>` line above it, or change the links too.
 */
const LINKED: Record<string, string[]> = {
  admin: [
    'overview', 'installation', 'docker-compose', 'from-source', 'first-start', 'headless',
    'command-line', 'server-roles', 'roles', 'other-commands', 'plugin-commands', 'user-commands',
    'configuration', 'core-settings', 'nats-output', 'control-plane', 'tls', 'engine-and-writer',
    'multi-node-sync', 'logging', 'test-only-variables', 'data-directory', 'redis', 'requirements',
    'users', 'first-admin', 'adding-accounts', 'disabling-accounts', 'passwords', 'api-tokens', 'revoking-sessions', 'users-panel',
    'sign-in', 'password-sign-in', 'saml', 'openstare-sign-in', 'client-certificates', 'sign-in-turned-off',
    'banners', 'classification-banner', 'notice-and-consent', 'security-labels', 'settings-tab',
    'sources-and-correlation', 'sources', 'output-schema', 'correlation-settings', 'tracker-profiles',
    'plugins', 'multi-node', 'tak-output', 'backup-and-restore', 'what-to-back-up', 'sqlite-backup', 'redis-backup',
    'configuration-export', 'registry-export', 'restore', 'upgrades', 'purge',
    'monitoring', 'health-checks', 'metrics', 'logs', 'troubleshooting', 'security', 'security-hardening',
  ],
  operator: [
    'getting-started', 'signing-in', 'roles', 'your-account', 'links', 'overview', 'sources',
    'correlation', 'suggestions', 'accept-or-reject', 'expired-suggestions', 'correlation-decisions',
    'track-management', 'track-map', 'bearings-and-areas', 'track-table', 'track-card', 'details-tab',
    'provenance-tab', 'confidence-and-existence', 'track-states', 'not-published', 'history-tab',
    'managing-tracks', 'pair', 'unpair', 'merge', 'split', 'do-not-pair', 'delete', 'groups', 'designate',
    'history-points', 'undo', 'management-log', 'export', 'registry', 'entities', 'entity-editor',
    'publish-override', 'spreadsheets', 'schema', 'settings', 'in-tak', 'common-questions',
  ],
}

describe('help anchors', () => {
  it('slugs headings as GitHub does', () => {
    expect(slugify('Security hardening (0.4.0)')).toBe('security-hardening-040')
    expect(slugify('Accept or reject')).toBe('accept-or-reject')
    expect(slugify('OT_AUTH off')).toBe('ot_auth-off')
  })

  it('takes an explicit anchor and numbers repeats', () => {
    const md = ['# T', '## A', '<a id="short"></a>', '', '## A long heading (1.0)', '```', '## not a heading', '```', '## A'].join('\n')
    expect(guideHeadings(md).map((h) => h.anchor)).toEqual(['t', 'a', 'short', 'a-1'])
    expect(renderableGuide(md).split('\n')).toHaveLength(md.split('\n').length)
  })

  it('links and parses help paths', () => {
    expect(helpHref('operator', 'merge')).toBe('#help/operator/merge')
    expect(helpHref('admin')).toBe('#help/admin')
    expect(parseHelpPath('admin/saml')).toEqual({ guide: 'admin', anchor: 'saml' })
    expect(parseHelpPath('nope')).toEqual({ guide: null, anchor: '' })
  })

  for (const g of GUIDES) {
    describe(g.file, () => {
      const md = guideMarkdown(g.id)
      it('is bundled', () => expect(md).toBeTruthy())
      const headings = guideHeadings(md ?? '')
      const anchors = headings.map((h) => h.anchor)

      it('has unique anchors, one title', () => {
        // No heading repeats another's slug (which would give it a `-1` anchor).
        for (const h of headings) expect(h.anchor === slugify(h.text) || (md ?? '').includes(`<a id="${h.anchor}"></a>`), h.text).toBe(true)
        expect(new Set(anchors).size).toBe(anchors.length)
        expect(headings.filter((h) => h.level === 1)).toHaveLength(1)
      })

      it('keeps every anchor tips link to', () => {
        for (const a of LINKED[g.id]) expect(anchors, a).toContain(a)
      })

      it('links only to anchors it has', () => {
        const other = new Map(GUIDES.map((x) => [x.file, guideHeadings(guideMarkdown(x.id) ?? '').map((h) => h.anchor)]))
        for (const [, file, anchor] of (md ?? '').matchAll(/\]\(((?:[a-z]+\.md)?)#([a-z0-9_-]+)\)/g)) {
          const pool = file ? other.get(file) : anchors
          expect(pool, `${file}#${anchor}`).toContain(anchor)
        }
      })
    })
  }
})
