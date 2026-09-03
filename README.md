# archietect

**The persistent architectural brain of a software project.**

[![CI](https://github.com/Neville777/Archietect/actions/workflows/ci.yml/badge.svg)](https://github.com/Neville777/Archietect/actions/workflows/ci.yml)
[![License: BSL 1.1](https://img.shields.io/badge/license-BSL--1.1-blue)](LICENSE)
[![Rust 2021](https://img.shields.io/badge/rust-2021-orange)](Cargo.toml)

A deterministic engine that maintains a living record of what a project's
concepts ARE — what exists, what is canonical, who uses it, why it is shaped
this way, and how that has changed over time. AI agents, editors, CI, and
humans all query the same continuously maintained state instead of each
reconstructing a partial, inconsistent understanding per session.

**Archietect does not use AI. AI uses Archietect.**

```
              Human
                │
      Claude / GPT / Cursor / Zed        ← intelligence lives HERE
                │
          Archietect (CLI / REST / MCP)
                │
        Architectural state              ← truth lives HERE
   laws · concepts · decisions · aliases
   evidence · provenance · history
                │
    source code · schemas · ADRs
```

## Demo

```
$ archietect concept doctor
{
  "concept": "doctor",
  "verdict": "STRUCTURAL",
  "canonical": "doctor",
  "confidence": "high — found in source as a real symbol, not a declared data/schema model",
  "evidence": [
    { "tier": "Declared", "what": "Function declared in src/query.rs:677" }
  ],
  "source": [{
    "file": "src/query.rs", "line": 677,
    "excerpt": "675: \n676: /// Repository summary for someone who just cloned it.\n677: pub fn doctor(idx: &Index, ...) -> Value {\n678:     // Domains = where declarations LIVE ..."
  }],
  "routes": [],
  "recommendation": "'doctor' exists in source but is not a schema/storage concept (it's a function, class, route, or similar). Schema-concept ranking does not apply."
}
```
(the line number above will drift as this file is edited — re-run it
yourself any time; it's live, not a fixture)

Real file, real line, real code excerpt, read fresh off disk at query time —
that's Archietect answering a question about its own source. Ask about
something that doesn't exist and it says so, at the same confidence, instead
of guessing:

```
$ archietect concept PaymentRefundService
{ "verdict": "ABSENT", "confidence": "high — no declaration, no observed usage, no name resemblance",
  "recommendation": "Genuinely new for this project. Building it is justified." }
```

And when a repo contains a language Archietect can't see into, it names that
gap instead of guessing past it:

```
$ archietect concept processOrder
{ "verdict": "INSUFFICIENT_COVERAGE",
  "confidence": "unknown — this repository contains files in a language with no structural extractor",
  "next_action": { "read": ["handler.lua"], "question": "Do any of these files implement or represent 'processOrder'?" } }
```

Every answer is one of exactly three things — **KNOWN** (a real answer, with
evidence), **KNOWN ABSENT** (searched everywhere it can see, genuinely isn't
there), or **INSUFFICIENT_COVERAGE** (a real blind spot, named honestly,
never silently guessed past). That's the entire trust model.

## Features

- **Evidence-tiered, never invented** — every answer is ranked `DECLARED`
  (the project's own schema asserts it) > `USED` (code observably touches
  it) > `NAMED` (name resemblance only — verdict `UNKNOWN`, needs a human).
- **Read-only against your code** — Archietect flags problems with evidence
  and stops; it never edits your working tree. The one exception (the
  proposal protocol, below) applies a patch a human explicitly accepted,
  uncommitted.
- **Fully offline and deterministic** — no AI inside, no API key, no
  network call. Intelligence stays in the client (Claude, GPT, Cursor, Zed).
- **Regression-tested against real repositories**, not just synthetic
  fixtures — see [Testing](#testing).
- **Append-only architectural history** — concept, alias, and decision
  changes are recorded as they happen and never rewritten.
- **Explicit blind spots** — a language with no structural extractor returns
  `INSUFFICIENT_COVERAGE`, never a false `ABSENT`, and names exactly which
  files a human or AI should read to check.
- **Three transports, one engine** — CLI, REST, and MCP all call the same
  query functions; no business logic lives in any one transport.

**Tech stack:** Rust, SQLite (`rusqlite`, bundled — no external DB to run),
regex-based structural/schema extraction, `tiny_http` for REST, stdio for MCP.

**Real, reproducible benchmark:** [archietect vs. Agent Memory Engine](benchmarks/vs-agent-memory-engine/) —
15/15 vs 5/15 on surfacing a concept's real declaring file, across 3 public
repos already in `validation/`. Every number is reproducible from a script
in that directory; limitations are stated there too.

## Structural coverage

| Language | Symbols | Frameworks (routes) |
|---|---|---|
| Rust | structs, enums, traits, top-level functions | — |
| Python | classes, top-level functions | FastAPI, Flask, Django |
| TypeScript/JavaScript | classes, interfaces, enums, exported functions, events | Express, NestJS, Next.js, Nuxt (server API) |
| Vue | the SFC itself as a component, plus its `<script>` block | Nuxt (pages) |
| Go | exported structs, interfaces, functions/methods | — |
| Java/Kotlin | classes, interfaces, Kotlin top-level functions | Spring MVC |
| Ruby | classes, modules, methods | Rails |
| Elixir | modules, public functions | Phoenix |
| PHP | classes, interfaces, top-level functions | — |
| C# | public classes/interfaces/records, public methods | ASP.NET Core |
| Swift | classes, structs, protocols, top-level functions | Vapor |
| Objective-C | `@interface`/`@implementation`, `@protocol`, methods | — |
| C/C++ | structs, functions; classes for `.cpp`/`.hpp` only | — |
| Scala | classes, objects, traits, top-level `def` | — |
| Dart | classes, top-level functions | — |
| Haskell | `data`/`newtype`, typeclasses, top-level signatures | Yesod (`parseRoutes` quasi-quote only) |
| Clojure | public `defn`, `defrecord`/`deftype`, `defprotocol` | Compojure |
| GraphQL | types/interfaces/enums/inputs, named operations | — |
| Protocol Buffers | messages, services, rpc methods (as routes) | gRPC |

The **schema layer** additionally recognizes storage declarations directly —
Prisma, Drizzle, TypeORM, Sequelize/Mongoose, Django, SQLAlchemy,
pydantic/SQLModel, Rails/ActiveRecord, Eloquent, JPA, GORM, Ecto, and raw
`CREATE TABLE` from any source.

Coverage is reported **per repository**: `archietect status`/`doctor`/`tour`
list exactly which languages and frameworks were found in *this* codebase,
so an `ABSENT` result is never a mystery. A language with no extractor at
all isn't guessed at either — `INSUFFICIENT_COVERAGE` names the gap and
lists the files worth reading.

Not attempted: a real parser (everything here is regex — an MVP tradeoff)
and route DSLs too combinator-heavy to track reliably (Servant's type-level
API, Akka HTTP's in-code routing) — a wrong route is worse than a missing one.

## Prerequisites

- No Rust toolchain needed if using a prebuilt binary (below).
- Building from source needs a stable Rust toolchain (`cargo build --release`).
- Linux daemon mode needs `systemd --user`; macOS daemon mode needs `launchd`
  (both installed via `packaging/onboard.sh --daemon`). Not available on
  Windows — the plain CLI/REST/MCP binary works anywhere.

## Installation

**Option A — install script**, no Rust toolchain needed:

```bash
curl -fsSL https://raw.githubusercontent.com/Neville777/Archietect/main/packaging/install.sh | sh

# once per project:
archietect init --root /path/to/your-project
claude mcp add archietect -- "$(which archietect)" mcp   # once, ever — every project reuses this
```

Downloads the right prebuilt binary for your platform (Linux x86_64, macOS
arm64/x86_64) from the latest GitHub Release. Windows and other platforms:
build from source (Option C). `--version vX.Y.Z` pins a version, `--dir`
picks the install directory — see the script's own header for both.

**Option B — `cargo install`**, if you already have a Rust toolchain:

```bash
cargo install archietect

archietect init --root /path/to/your-project
```

**Option C — build from source** (needed for `packaging/onboard.sh`'s
one-command flow, the systemd/launchd daemon install, or running the test
suite):

```bash
git clone git@github.com:Neville777/Archietect.git archietect && cd archietect
cargo build --release

# once per project you want Archietect to understand:
packaging/onboard.sh /path/to/your-project
```

`onboard.sh` builds (if needed), indexes the project, registers the MCP
server **globally** — one registration, every onboarded project on the
machine becomes queryable by any MCP-speaking AI tool — and ends with a
readiness report (real output, a two-file FastAPI+Prisma project):

```
╭──────────────────────────────────────────╮
│            ARCHIETECT READY                 │
╰──────────────────────────────────────────╯

Architecture
  Files      2
  Symbols    1
  Routes     1
  Concepts   1
  Laws       14

Structural coverage (what Archietect can actually see in THIS repo)
  Python  (1 files) — classes, top-level functions, routes

Integrations
  ✓ CLI
  ✓ MCP
  ○ Watch daemon
```

Add `--daemon` to also install an always-on watcher (systemd `--user` on
Linux, `launchd` on macOS) so the index stays warm and architectural events
are recorded to history as they happen, instead of being recomputed cold on
every query.

## Usage

From inside any onboarded project — no `--root` needed, it walks upward
looking for `archietect.db`, the same way git finds `.git`:

```bash
archietect                    # git-status-style glance
archietect status             # what's declared, used, and — per coverage — visible at all
archietect concept <name>     # does X exist, where, what's the evidence
archietect impact <name>      # what breaks if X changes
archietect duplicates         # suspected redundant concepts, before you add a new one
```

Full command reference:

| Command | Purpose |
|---|---|
| `archietect init --root DIR` | build/refresh `archietect.db` |
| `archietect status --root DIR` | what's declared, used, and structurally visible |
| `archietect concept --root DIR TERM` | does `TERM` exist? canonical? evidence? |
| `archietect intent --root DIR "GOAL"` | smallest correct change: EXTEND vs CREATE |
| `archietect plan --root DIR "GOAL"` | one-call composition of concept+owner+impact+decisions |
| `archietect impact --root DIR TERM` | what is affected if `TERM` changes |
| `archietect owner --root DIR TERM` | which directory owns `TERM`'s declaration |
| `archietect guard --root DIR "SQL"` | rejects `CREATE TABLE` duplicating a concept |
| `archietect doctor --root DIR` | repository summary for someone who just cloned it |
| `archietect tour --root DIR` | onboarding: what matters, what's ignorable, past mistakes |
| `archietect duplicates --root DIR` | suspected redundant concepts — risk, not proof |
| `archietect verdicts --root DIR` | every declared concept bucketed by verdict (ACTIVE vs DECLARED_ONLY), project-wide |
| `archietect register --root DIR [--since-last]` | the map of the bag: what's known, not known, and why — see below |
| `archietect history --root DIR [TERM] [--digest]` | the architectural timeline (what git can't say); `--digest` narrates it instead of listing raw events |
| `archietect concept-at --root DIR TERM --version N` | episodic replay: what `TERM` looked like at a past architecture version (needs `watch` to have run) |
| `archietect seed --root DIR [--write] [--proposed-by WHO]` | cold-start fix: propose `[[decision]]` entries from README.md bullet points, verbatim |
| `archietect history-archive --root DIR --before-days N` | move old events into a permanent archive file — never deletes |
| `archietect ci` | pipe a diff in, get an exit code out |
| `archietect laws` | the language specification, from `laws/*.toml` |
| `archietect watch --root DIR` | daemon: observe → notify, never act |
| `archietect serve --port 7373` | REST API (127.0.0.1, read-only except `/proposal/*`) |
| `archietect mcp` | MCP server over stdio |
| `archietect proposal submit\|list\|inspect\|test\|accept\|reject` | the AI-extension protocol |
| `archietect permissions[-check] --root DIR` | the domain permission boundary, and whether one path is allowed |
| `archietect docker observe --root DIR` | LIVE container state via `docker compose ps` — explicit, opt-in, never automatic |
| `archietect documents\|photos scan --root DIR --dir PATH` | the unstructured domains — metadata only, content never read |
| `archietect system register\|list\|status\|query TERM` | the cross-project registry (`~/.archietect/system.db`) |

### Clients: the same engine, three transports

| | Use | Command |
|---|---|---|
| CLI | scripting, terminal, CI | `archietect <cmd>` |
| REST | GUI, dashboards, anything HTTP-shaped | `archietect serve --port 7373` (127.0.0.1 only) |
| MCP | every AI coding tool | `archietect mcp` (stdio) |

Long-running processes (MCP, REST, `watch`) detect if the binary on disk has
been rebuilt out from under them since they started, and return a
`_stale_binary_warning` instead of silently answering from stale in-memory
code — restart the process/session to clear it.

### The per-project ontology: `archietect.toml`

```toml
[aliases]
episode = "stories"        # the concept exists under a different name —
                           # what no name search can ever see

[[decision]]               # ADRs: the WHY, with the roads not taken
id = "stories-own-episodes"
decision = "Episodes are stored as stories"
because = "they always shared identity"
rejected = ["separate episodes table"]   # ← what the next person will propose
links = ["episode", "stories"]
```

`archietect guard` cites the governing decision when it rejects — "this table
already exists" states a fact; the decision states the reasoning, which is
what stops the same proposal returning next month under a different name.

### Memory model

Projects never share architectural memory, and no per-project state ever
leaves that project's own directory:

```
Archietect core (compiled into the binary)
  └── laws/*.toml — universal rules about how the engine matches and ranks,
      the same for every project, never copied anywhere

Each project — <root>/archietect.db (one SQLite file)
  ├── architecture state (concepts, structural graph)
  ├── decisions + aliases (mirrored from that project's own archietect.toml)
  └── immutable event history (append-only)
```

A developer's `~/storefront/archietect.db` and another developer's
`~/payments/archietect.db` are independent memories governed by the same
compiled-in laws. `init`/`save` only ever `INSERT OR REPLACE` known keys and
`CREATE TABLE IF NOT EXISTS` — re-running `init` (or the onboarding script)
against a project can never drop its history or decisions.

## Contributing: the proposal protocol

The only door through which a change — a new structural extractor, or a new
`archietect.toml` decision/alias — can reach a repository, and it never opens
on its own. An AI proposes work, never evidence: `Tier::Inferred` does not
exist.

```bash
archietect proposal submit --kind extractor|decision|alias \
    --title "..." --patch some.diff      # inert patch, nothing applied yet
archietect proposal test <id>             # applies it in an isolated git worktree
                                          # and runs it through the SAME laws +
                                          # invariants suite — the real
                                          # working tree is never touched
archietect proposal accept <id>           # only if: status == passed, the patch
                                          # is byte-identical to what was
                                          # tested, AND the repository HEAD
                                          # hasn't moved since — then applies to
                                          # the real working tree, UNCOMMITTED.
                                          # Archietect never runs `git commit`.
```

`check_scope()` hard-blocks any patch that touches the validation machinery
itself (`laws.rs`, `tests/laws.rs`, `tests/invariants.rs`, `store.rs`,
`model.rs`, `proposal.rs`, `Cargo.*`, `.github/`) or strays outside its
kind's allow-list — a proposal cannot weaken the suite it's judged by.
Reachable over CLI, REST (`/proposal/*`), and MCP
(`proposal_submit`/`proposal_test`/...) — the same trust boundary regardless
of which client is holding the pen.

`laws/` is walled off from this protocol entirely — no proposal, from any
user, on any install, can create or edit a law; only a human maintainer, in
a real release, can. Found a genuine defect in Archietect itself (not a
local coverage gap)? File it: https://github.com/Neville777/Archietect/issues.

## Testing

Two suites, different guarantees:

- `tests/laws.rs` — one synthetic fixture per law, each tied to a specific
  bug this engine previously produced on a real repository.
- `tests/invariants.rs` — real cloned open-source repositories (chatwoot,
  lobe-chat, umami, Saleor, BookStack, dub, analytics, redash, Rails' and
  NestJS's own RealWorld ("Conduit") implementations for the schema layer;
  ASP.NET Core, C, Dart, Scala, Nuxt's own devtools monorepo, and gRPC's own
  canonical examples for structural-only checks, routes included) — proves
  the bug class can't occur in any scanned corpus, not just the repo that
  first surfaced it.

**Laws:** 15 active. A law states a timeless claim about what Archietect is
allowed to assert — e.g. `law-015`: "must not return a confident `ABSENT`
when coverage is insufficient" — separate from *how* today's code enforces
that claim, which can change without the law itself changing.
`conformance_registry_matches_suite` enforces both directions: a law without
a covering test fails CI, and a test claiming a law the registry doesn't
know fails too. New laws are minted only for genuinely new invariants; an
incident that's really another instance of an existing invariant gets a new
regression fixture attached to that law instead.

As a user, none of this is required reading — the three verdict states
(KNOWN / KNOWN ABSENT / INSUFFICIENT_COVERAGE) are the entire interface, the
same way you don't need to know which regression test a compiler runs to
trust that it compiles your code correctly. This section documents how
Archietect is *developed*, so it keeps improving release over release
without regressing.

## License

[Business Source License 1.1](LICENSE) — source-available, not permissive
open source. In plain language: free to read, run, modify, and use in
production — including at work, on your employer's codebases, as an
employee or paid contractor — for anything except turning Archietect itself
into a competing commercial offering (reselling it as a hosted/managed
service, or bundling it into a paid developer-tools product) without a
separate commercial license. Converts automatically to Apache License 2.0
(fully open source) on 2030-09-01, or sooner if a future release sets an
earlier date.

For a commercial license, or any other licensing question: nevillejemo@gmail.com.
