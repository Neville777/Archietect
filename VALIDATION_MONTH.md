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

## Post-freeze queue (canonicalized — do not build twice)

ONE deferred feature, proposed twice under different names and unified here
before the duplication could happen (the tool's own discipline, applied to
its own roadmap):

**The decision-event** — a human ruling on an architectural question,
recorded in the SAME append-only timeline the daemon writes. Covers both
proposals: the "explanation recorder" (guard advice → followed/overridden +
override reason) and the "runtime decision record" (merged X into Y, by
whom, why, evidence). Same envelope, same storage, same archaeology
queries. `architect history report` then answers WHY, not just what.
Guard-outcome events are the advice-given view; merge/rename rulings are
the action-taken view. Build once. Gated on the month proving the advice
is consulted at all.

**Cross-repository search** (`architect search User` across the org, with
consolidation recommendations) — post-freeze Track, explicitly deferred
twice now; single-repo value must be proven first.

**`architect law new`** — interactive scaffolder (what happened → what was
wrong → law toml + fixture + regression skeleton). Serves the maintainer,
not the month; post-freeze.

**TITAN internal/standalone unification** — TITAN's repair loop already
passes through its INTERNAL guard (architecture.rs, wired 2026-08-05); the
standalone engine serves this machine via MCP. One engine serving both is
post-freeze consolidation — and must itself pass `architect concept guard`
first, because two guards is a duplicate concept.

**The taxonomy rename** (`laws/` → `engine semantics / evidence model /
policy / regression / explanation`) — proposed three times now (2026-08-05
×2, 2026-08-06), deferred each time. Verdict unchanged: no bug requires
it, it's mid-freeze churn on working, fixture-enforced code, and each
proposal has arrived as pure taxonomy with no new behavior attached. NOT
untrue, though — `scoring.rs` (built 2026-08-06, credited above) already
implements MOST of the "evidence model" half: `RankTier::base_score()` is
a precedence-per-source ordering (alias > ORM > SQL > usage), which is
exactly the `Evidence { source, precedence }` shape proposed. The one real
gap: `resolve_alias()` bypasses the lattice entirely as a special-cased
early check (law-011's fix) rather than `DeclaredOntology` being a normal
tier the sort naturally selects — structurally fine (correct answers,
verified), but not yet the "one ranking path" purity the vision wants.
TRIGGER for finally doing this: batch it with the decision-event work
above — both touch query.rs's core paths, so one pass, one round of
re-verification, not two. Do not start it opportunistically mid-freeze
just because it keeps being proposed.

## The five-year frame (recorded 2026-08-06, not enacted)

"Architect becomes the compiler for architecture" — every client (CLI,
MCP, REST, GUI, CI, IDE, TITAN) asks, none compute; the daemon is the one
writer, the product, and eventually invisible the way git/rust-analyzer
are invisible. Matches what's already built (one engine, thin transports,
read-only clients) — nothing here contradicts current architecture, it's
a naming of the destination already being walked toward.

**Post-freeze success metrics (replaces commit/law/corpus counts as the
headline, once the month ends):**
1. Architectural consistency — duplicate concepts / conflicting decisions
   actually prevented, not just detectable.
2. Time to architectural answer — `architect owner X` vs. manually
   grepping, measured, not assumed.
3. Trust — recommendations accepted without independent verification.
   (Needs the decision-event recorder above to measure at all — currently
   0 of these 3 are instrumented; commit/law/corpus counts remain the
   ONLY measured numbers until then, and diary entries should keep saying
   so rather than implying otherwise.)

**Distribution roadmap (not started):** single binary, subcommands own
daemon/mcp/serve/ci/watch (no separate installs to explain — already true
today); package managers (cargo install today; brew/apt/winget later);
`architect bootstrap` one-shot init+daemon-enable; framework integration
(`cargo new` / `npx create-next-app` prompting to enable it) is a
multi-year distribution bet, not a near-term item.

**`architect explain <concept>`** — a NARRATIVE renderer over facts that
already exist (provenance, relations, rejected duplicates, laws cited,
decisions). No model, no generation: sentence templates over the same JSON.
Six-month item; it becomes worth building only when decision-events exist
to narrate — explain without archaeology is a paraphrase of the concept
card.

## Day-1 baseline (Track 3 — evidence, not marketing)

Measured 2026-08-05, 12 repos on this machine. Cold = full init; warm =
status via incremental cache; db = architect.db on disk.

| repo | concepts | used | cold scan | warm query | db size |
|---|---|---|---|---|---|
| umami | 19 | 18 | 85ms | 40ms | 160K |
| dub | 134 | 75 | 631ms | 171ms | 804K |
| saleor | 131 | 82 | 785ms | 107ms | 936K |
| redash | 22 | 16 | 98ms | 41ms | 136K |
| chatwoot | 164 | 64 | 2487ms | 204ms | 588K |
| lobe-chat | 149 | 132 | 2393ms | 369ms | 1.8M |
| BookStack | 53 | 36 | 334ms | 56ms | 324K |
| analytics | 54 | 19 | 324ms | 80ms | 236K |
| spring-petclinic | 7 | 7 | 56ms | 15ms | 36K |
| backend | 198 | 132 | 1081ms | 243ms | 340K |
| ghosttrack-monorepo | 61 | 44 | 884ms | 93ms | 200K |
| qa-studio | 127 | 86 | 1335ms | 188ms | 1.2M |

Not yet measurable (recorder deferred): duplicates prevented, CI failures
prevented, overrides, false positive/negative rates — those need the month
and the decision-event recorder.

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
- 2026-08-05 (day 1, cont.) — recorded 10 family decisions in TITAN's
  architect.toml (events/state/ledger/referral/history + snapshots/domain/
  holon/transactions/access), evidence-checked: the deposit_events pair is
  TWO CHAINS (naming asymmetry = deliberate debt), swarm_vitality_ledger
  has NO WRITER (suspected orphan, recorded do-not-extend). The dormant
  domain_*/holon_* truth is now in the ontology, out loud. Kernel check:
  architecture:duplicate_concepts carries 97 belief_events — the drive
  loop revised it every beat all day. Corpus crank: 4 ecosystems in one
  pass (Eloquent/JPA/gorm/Ecto, corpus at 18). My guard-test expectation
  was wrong once: CREATE TABLE of the canonical's own declared table is a
  MIGRATION (law-002 exemption), not a duplicate — the engine was right
  and the tester was not.
- 2026-08-05 (day 1, cont.) — `architect plan` (pure composition, glance
  precedent): one call = intent+owner+impact+decisions; first TITAN run
  cited 'referral-tables-are-funnel-stages' recorded an hour earlier — the
  decisions loop closed same-day. LAW-010 STRUCK TWICE: plan() passed
  canonical names through term search (owner/impact null) — law generalized:
  a known concept name is an exact key EVERYWHERE; term search is for human
  input only. Baseline table recorded (12 repos: cold 56ms–2.5s, warm
  15–369ms, db 36K–1.8M).
- 2026-08-05 (day 1, owner's first real-world use) — owner ran it on a WORK
  repo (Coseke onboard-v1) and filed the first outside bug report: bare
  `architect` infers root but every subcommand demanded --root. Fixed
  git-style: ONE resolver before dispatch, --root optional everywhere.
  The fix found two deeper bugs in sequence: (1) weak markers (Cargo.toml)
  LIE in workspaces — resolver stopped at crates/titan_api and confidently
  answered for one crate; strong markers (architect.db/.toml/.git) now beat
  weak at any distance. (2) THE READ THAT WROTE: read_history opened
  SQLite without an existence check, and SQLite creates on open — the
  glance left 0-byte architect.db droppings wherever it ran, and each
  dropping became a STRONG root marker poisoning discovery. Reads now open
  SQLITE_OPEN_READ_ONLY; droppings cleaned. A read-path that writes
  corrupts more than the principle — it corrupted navigation.
- 2026-08-06 (day 2) — root discovery split into its own subsystem
  (root.rs), wired through CLI+REST+MCP identically. DISCOVERED: a
  concurrent session (Kiro) had been working in the same uncommitted
  working tree — scoring.rs (precedence lattice), invariants.rs (4
  corpus-wide structural checks, 8/8 green), a new rust pub-struct
  extractor. Independently re-verified all of it before trusting it (full
  checklist re-run against live repos). VERIFICATION EARNED ITS KEEP:
  found LAW-011 — the new rust extractor made GameTheoryEngine (real
  TITAN code) visible, and it silently defeated the declared
  theory=causal_hypotheses alias, because alias resolution only ran when
  name-token search came up EMPTY — contradicting scoring.rs's own tier
  lattice. Fixed + fixture + EXTRACTOR_VERSION bump. Second same-class bug
  found by USE, not by design review: the ordering gap was invisible until
  a real codebase produced the exact token collision.
- 2026-08-06 (day 2, cont. — "test, dogfood") — full test suite (23/23)
  and live dogfooding on both architect's own repo and TITAN. Three real
  bugs found and fixed, all by USE, none by review:
  (1) architect's own architect.toml didn't exclude tests/fixtures/ — the
      fixture dirs (deliberately containing collisions like Ghost/ghosts)
      were scanned as real source, reporting 3 fake duplicate risks on
      itself. Fixed; 27→14 real concepts, 0 fake risks.
  (2) LAW-012: `architect concept ScoreBreakdown` — the exact, correct,
      full name of a real struct — returned ABSENT. names_concept() only
      compared TOKENS of a name against the query term, never the whole
      name; a multi-token name could never match its own literal spelling.
      The worst possible failure mode (confident false ABSENT). Fixed +
      fixture. Also caught my own gap building it: registered the law
      before writing its test — conformance passed vacuously on the
      string match while the fixture sat unused.
  (3) The new rust extractor's 3,838-concept surface broke two features
      silently: family suggestions drowned in 294 *Config/108 *Result/93
      *Response (universal naming, zero duplication cost), and
      duplicates() — O(n^2) — took 23s on TITAN, which bare `architect`
      calls. Both restricted to storage-bearing (table.is_some())
      concepts: signal restored, 23s→11.6s (isolated: that remainder is
      cold-scan cost, not the bug — warm/incremental is 1.5s).
  TITAN dogfood, clean: guard still blocks CREATE TABLE episodes citing
  the ADR; concept theory still resolves via alias through the fixed
  ordering; 12/12 corpus canonical picks unchanged throughout.
- 2026-08-06 (day 2, cont.) — "keep dogfooding: run architect watch on
  TITAN for a real stretch." This was the highest-value finding of the
  day. Two SEVERE, separate daemon bugs, both making it unusable at
  TITAN's scale, NEITHER the bug I expected going in:
  (1) `watcher.watch(&root, RecursiveMode::Recursive)` registers ONE
      recursive watch on the whole tree — including target/ (15GB, 3,430
      dirs on TITAN). Registration alone: unmeasured, still running
      after 2+ minutes, killed. Fixed: walk the tree respecting the same
      SKIP_DIRS the scanner uses, register non-recursive watches per
      directory. 545 real dirs, 10s startup.
  (2) The real bug, and NOT the O(n^2) rename-detection I suspected
      (instrumented and proved it: 20ms, fine). SQLite's rollback-journal
      file is created+deleted around every write; `relevant()`'s exact-
      filename check let both events through, so every daemon WRITE
      generated a new event that WOKE the daemon, which wrote again —
      fully self-sustaining. Observed: 1,300+ events drained every 300ms,
      100% CPU, 4+ minutes, killed. Fixed: prefix match, not exact.
  Verified: single touch -> exactly one 1.3s rescan -> daemon genuinely
  idle (STAT=Sl) at 3 checkpoints over 20+ seconds. This was blocking —
  the setup instructions tell the owner to enable architectd@; without
  today's fix, that would have pegged a CPU core indefinitely on the
  first real edit. Full regression: 23/23 tests, 12/12 corpus unchanged.
