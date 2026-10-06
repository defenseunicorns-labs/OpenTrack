# OpenTrack UI

React + TypeScript (Vite) on [stareSDK](https://github.com/kineticlogic-io/stareSDK) (`@kineticlogic/staresdk`). See the repository README for
running it; `npm run dev` proxies `/api` to `opentrack serve` on :8090 (override with
`OT_API_URL`).

Rules: reusable components come only from `@kineticlogic/staresdk`; page layout uses
stareSDK tokens (`var(--…)`) only; icons come only from `react-icons/tb`. A missing component is
proposed for stareSDK upstream rather than built here.
