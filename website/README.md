# archietect website

The project landing page — Next.js (App Router), deployed independently
from the Rust engine in the rest of this repo.

```bash
npm install
npm run dev    # http://localhost:3000
npm run build  # production build, used by Vercel
```

## Deploying

Point a new Vercel project at this repo with **Root Directory** set to
`website/` — Vercel auto-detects Next.js and needs no other configuration.

## Content

Every fact on the page (the benchmark numbers, the language coverage
table, the demo JSON, the pull quotes) is copied from this repo's own
[README.md](../README.md) and [AI_AGENT_REPORT.md](../AI_AGENT_REPORT.md) —
update those first, then bring the matching text over here by hand; there's
no shared data source between the two yet.
