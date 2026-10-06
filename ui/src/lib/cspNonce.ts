import { StyleModule } from 'style-mod'

/**
 * The page's content security policy allows a `<style>` element only when it carries this page
 * load's nonce, which the server puts in `<meta property="csp-nonce">` (a fresh one each load).
 * Empty in development (`npm run dev`), where there is no policy.
 */
export function cspNonce(): string {
  const meta = document.querySelector<HTMLMetaElement>('meta[property="csp-nonce"]')
  // Browsers hide a nonce attribute once the page has loaded; the property keeps it.
  const nonce = meta?.nonce || meta?.getAttribute('nonce') || ''
  return nonce.startsWith('__') ? '' : nonce
}

/**
 * Give the nonce to everything that adds `<style>` elements, before any of it runs:
 * - the code editors (CodeMirror, through style-mod): the page's one style-mod element is made
 *   now, with the nonce, and every editor after adds its rules to it;
 * - any other `<style>` a script of this page makes (the SDK's Help page prose style, which takes
 *   no nonce): `document.createElement('style')` gives it the nonce. Markup injected into the
 *   page is not made this way, so an injected `<style>` or `style` attribute is still refused.
 * The animations (framer-motion) take it through `MotionConfig` (main.tsx).
 */
export function applyCspNonce(nonce: string) {
  if (!nonce) return
  StyleModule.mount(document, new StyleModule({}), { nonce })
  const create = document.createElement
  document.createElement = function (this: Document, tag: string, options?: ElementCreationOptions) {
    const el = create.call(this, tag, options)
    if (el instanceof HTMLStyleElement) el.setAttribute('nonce', nonce)
    return el
  } as typeof document.createElement
}
