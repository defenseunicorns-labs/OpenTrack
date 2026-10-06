import react from '@vitejs/plugin-react'
import { defineConfig, searchForWorkspaceRoot } from 'vite'

// In development the API runs separately (`opentrack serve`, port 8090);
// in production the server serves this build at `/`.
const api = process.env.OT_API_URL ?? 'http://127.0.0.1:8090'

export default defineConfig({
  plugins: [react()],
  // The Sources page chunk carries CodeMirror and MapLibre (~290 kB gzipped); it is loaded
  // lazily, only when that page opens.
  build: { chunkSizeWarningLimit: 1200 },
  // The server puts a fresh nonce here in each page it serves, and in the page's content security
  // policy (crates/ot-server/src/control.rs): the page's <style> may run, an injected one not.
  html: { cspNonce: '__OT_CSP_NONCE__' },
  server: {
    host: '0.0.0.0',
    // xfwd: the server's cross-site check sees the page's own address (X-Forwarded-Host).
    proxy: { '/api': { target: api, changeOrigin: true, xfwd: true }, '/healthz': api },
    // The Help page bundles the guides from ../docs/guides (see src/pages/help/guides.ts).
    fs: { allow: [searchForWorkspaceRoot(process.cwd()), '../docs/guides'] },
  },
})
