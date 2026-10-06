import { StrictMode } from 'react'
import { MotionConfig } from 'framer-motion'
import { createRoot } from 'react-dom/client'
import '@fontsource/montserrat/400.css'
import '@fontsource/montserrat/500.css'
import '@fontsource/montserrat/600.css'
import '@fontsource/montserrat/700.css'
import '@fontsource/oswald/500.css'
import '@fontsource/oswald/600.css'
import 'maplibre-gl/dist/maplibre-gl.css'
import '@kineticlogic/staresdk/styles.css'
import { ThemeProvider, ToastProvider } from '@kineticlogic/staresdk'
import './index.css'
import { AuthProvider } from './auth/AuthProvider'
import { Root } from './auth/Root'
import { applyCspNonce, cspNonce } from './lib/cspNonce'

// Before anything adds a <style> element (see lib/cspNonce.ts).
const nonce = cspNonce()
applyCspNonce(nonce)

// After a deploy, a page loaded earlier asks for code chunks the server no longer has. Reload once
// to pick up the new build (the guard stops a loop if loading keeps failing for another reason).
window.addEventListener('vite:preloadError', (event) => {
  let reloaded: string | null = null
  try {
    reloaded = sessionStorage.getItem('ot.reloadedForBuild')
  } catch {
    /* storage unavailable: reload anyway */
  }
  if (reloaded && Date.now() - Number(reloaded) < 30_000) return
  try {
    sessionStorage.setItem('ot.reloadedForBuild', String(Date.now()))
  } catch {
    /* ignore */
  }
  event.preventDefault()
  window.location.reload()
})

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <MotionConfig nonce={nonce || undefined}>
      <ThemeProvider>
        <ToastProvider>
          <AuthProvider>
            <Root />
          </AuthProvider>
        </ToastProvider>
      </ThemeProvider>
    </MotionConfig>
  </StrictMode>,
)
