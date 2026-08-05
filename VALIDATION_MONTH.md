# The validation month — feature freeze, declared 2026-08-05

Architectural features are FROZEN. The engine is internally coherent: every
component is a client of the same deterministic core, and the next six
months of investment hinge on evidence, not on another capability. The
question changed from "what should Architect do?" to "what evidence do we
need?" — this file is the protocol and the lab notebook.

## The three validation questions

1. **Does it change developer behavior?** Duplicate concepts prevented,
   guard rejections, recommendations followed vs overridden.
2. **Does it survive codebases its author didn't design?** The extractor
   loop continues (Laravel, JPA, gorm, Ecto are corpus work, not features —
   each ecosystem is exhausted before anything new is built).
3. **Does it disappear?** Success = nobody opens Architect; AI, CI and
   editors ask it silently, and humans notice only when something is wrong.

## Setup (owner's actions — each is persistent config, so each is yours)

```bash
# 1. every AI session on this machine becomes a client
claude mcp add --scope user architect -- \
  /home/nevo/Personal_Projects/architect/target/release/architect mcp

# 2. the daemon runs at login for each active repo
cp packaging/architectd.service ~/.config/systemd/user/architectd@.service
systemctl --user enable --now architectd@$(systemd-escape /home/nevo/Personal_Projects/universal_trader/backend)

# 3. the CI line, wherever a pipeline exists
git diff main... | architect ci --root .
```

## What is measurable TODAY (no new code)

| Metric | Source |
|---|---|
| Architectural events (drift, renames, versions) | `architect history` — append-only timeline |
| Guard verdicts in CI | pipeline logs (exit codes are the record) |
| Corpus growth, laws learned | `architect laws` (`laws/corpus.toml`) |
| Concepts believed / stopped believing | `architect status` per repo, over time |

## What is NOT measurable today — deliberately deferred

Consultation counts (how often AI called `concept`), recommendation
outcomes (followed vs overridden, and WHY overridden) are not recorded
anywhere. Recording them is the **explanation recorder** — the first
post-freeze feature, and only if the month proves the tool earns it.
Deferred on purpose: queries are read-only by design, and instrumenting
advice before knowing anyone takes the advice is measuring the mirror.
The override-reason field is the valuable half: "overridden — new bounded
context" teaches more than any acceptance count.

## The bar

> "It feels strange to work in a repository without Architect running."

If that sentence is true in a month, the transition from project to
infrastructure happened. If it is not true, no feature would have made it
true, and the month found that out at the cheapest possible price.

## Diary — every time it helped, or got in the way

(append entries here; honesty about "got in the way" is the point)

- 2026-08-05 — freeze declared. 19 commits, 10 extractors, 9 laws,
  14-repo corpus, all interfaces built through GUI v0. TITAN registered as
  first client (architect.toml committed in its repo).
- 2026-08-05 (day 1, full TITAN battery) — HELPED: episode/situation/theory
  all resolve through the ontology; guard blocks episodes citing the ADR;
  intent("per country phenomenon coverage") → extend phenomena, nothing new;
  ci caught a repair-loop-style diff adding CREATE TABLE experiments (exit 1,
  cited belief_experiments); duplicates surfaced the LEDGER FAMILY —
  action_ledger/detector_ledger/reality_ledger/spend_ledger_ts, four ledgers,
  a real TITAN finding worth an owner decision. GOT IN THE WAY / WRONG:
  (1) theory→UNKNOWN — alias targets fed through term search broke on
  multi-token names → LAW-010 + fixture, fixed same day; (2) owner→'crates'
  — monorepo containers are not owners → two-segment rule; (3) owner then
  picked titan_api (3 readers) over titan_knowledge (1 declaration),
  contradicting the stated principle → owner now comes from DECLARING
  directories only, usage breaks ties. 11/11 laws green after all three.
- 2026-08-05 (day 1, cont.) — owner codes terminal-only, no IDE: bare
  `architect` now prints human text by default (--json for scripts;
  subcommands stay JSON for jq). Size-sorted family suggestions immediately
  deepened the findings: not 4 ledgers but FIVE (swarm_vitality_ledger),
  plus EIGHT *_events tables and SIX *_state tables, no governing decisions.
  Alphabetical truncation had buried the biggest family — presentation
  order is epistemics too.
