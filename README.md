# architect

**The persistent architectural brain of a software project.**

Not an AI tool. Not code search. Not a linter. A deterministic engine that
maintains a living record of what a project's concepts ARE — what exists,
what is canonical, who uses it, why it is shaped this way, and how all of
that has changed over time. AI agents, editors, CI, and humans are all
clients of the same continuously maintained state, instead of each
reconstructing a partial, inconsistent understanding per session.

The inversion is the whole invention: **Architect does not use AI. AI uses
Architect.**

```
              Human
                │
      Claude / GPT / Cursor / Zed        ← intelligence lives HERE
                │
          Architect (CLI / REST / MCP)
                │
        Architectural state              ← truth lives HERE
   laws · concepts · decisions · aliases
   evidence · provenance · history
                │
    source code · schemas · ADRs
```

## See it work

```
$ architect concept doctor
{
  "concept": "doctor",
  "verdict": "STRUCTURAL",
  "canonical": "doctor",
  "confidence": "high — found in source as a real symbol, not a declared data/schema model",
  "evidence": [
    { "tier": "Declared", "what": "Function declared in src/query.rs:662" }
  ],
  "source": [{
    "file": "src/query.rs", "line": 662,
    "excerpt": "661: /// Repository summary for someone who just cloned it.\n662: pub fn doctor(idx: &Index, ...) -> Value {\n663:     // Domains = where declarations LIVE ..."
  }],
  "routes": [],
  "recommendation": "'doctor' exists in source but is not a schema/storage concept (it's a function, class, route, or similar). Schema-concept ranking does not apply."
}
```

That's Architect answering a question about its own source — real file, real
line, real code excerpt, read fresh off disk at query time. Ask about
something that genuinely doesn't exist and it says so, with the same
confidence, instead of guessing:

```
$ architect concept PaymentRefundService
{ "verdict": "ABSENT", "confidence": "high — no declaration, no observed usage, no name resemblance",
  "recommendation": "Genuinely new for this project. Building it is justified." }
```

## Founding principles

1. **The Architect never invents architectural facts.** Every answer carries
   evidence, tiered by strength: `DECLARED` (the project's own schema asserts
   it) > `USED` (code observably touches it) > `NAMED` (resemblance only —
   verdict UNKNOWN, needs human confirmation). A confident answer built on
   weak evidence is worse than grep, because it looks like knowledge.
2. **The Architect never modifies code. Ever.** It says *this is wrong, here
   is why, here is the evidence* — and stops. Its authority comes from being
   the source of truth, not from being another actor making changes. The
   guard rejects; it never rewrites. Even the one mechanism that DOES touch
   the working tree (the proposal protocol, below) never invents an
   answer — it applies a patch a human explicitly accepted, uncommitted.
3. **No AI inside, no API key, no network.** The core is offline and
   deterministic, like git. Intelligence belongs in the clients.
4. **Laws, not heuristics.** Every wrong answer on a real repository becomes
   a law (`laws/*.toml`) with a permanent fixture (`tests/fixtures/law_NNN/`)
   enforced by `cargo test`. Laws are amended, never edited away — semantics
   are versioned. A law without its regression test is a wish, and the wish
   fails CI.
5. **History is append-only.** The daemon records architectural events
   (concepts appearing, renaming, losing storage; aliases and decisions
   changing; version bumps). History that can be rewritten is not history;
   subscription filters shape the stream, never the record.
6. **Fewer believed concepts can be better.** The proudest metric so far is
   a concept count going DOWN (204 → 198): six phantom concepts eliminated.
   The engine celebrates stopping believing lies, not believing more things.
7. **Absence of evidence is disclosed, never hidden.** If part of a repo is
   in a language Architect can't see into, it says `INSUFFICIENT_COVERAGE`
   instead of a false `ABSENT` — and hands back exactly which files a human
   or an AI would need to read to actually check.

## Getting started

Requires Rust (`cargo build --release` — the binary is dependency-free
after that, no runtime, no API key).

```bash
git clone <this repo> architect && cd architect
cargo build --release

# once per project you want Architect to understand:
packaging/onboard.sh /path/to/your-project
```

That one script builds (if needed), indexes the project, registers the MCP
server **globally** — one registration, every onboarded project on the
machine becomes queryable by any MCP-speaking AI tool — and ends with a
readiness report (real output, a tiny two-file FastAPI+Prisma project):

```
╭──────────────────────────────────────────╮
│            ARCHITECT READY                 │
╰──────────────────────────────────────────╯

Architecture
  Files      2
  Symbols    1
  Routes     1
  Concepts   1
  Laws       14

Structural coverage (what Architect can actually see in THIS repo)
  Python  (1 files) — classes, top-level functions, routes

Integrations
  ✓ CLI
  ✓ MCP
  ○ Watch daemon
```

`packaging/onboard.sh --daemon` additionally installs an always-on watcher
(systemd `--user` on Linux, `launchd` on macOS) so the index stays warm and
architectural events get recorded to history as they happen, instead of
being recomputed cold on every query.

From inside any onboarded project — no `--root` needed, it walks upward
looking for `architect.db`, the same way git finds `.git`:

```bash
architect                    # git-status-style glance
architect status             # what's declared, used, and — per coverage — visible at all
architect concept <name>     # does X exist, where, what's the evidence
architect impact <name>      # what breaks if X changes
architect duplicates         # suspected redundant concepts, before you add a new one
```

## Structural coverage

Two layers see different things. The **schema layer** recognizes storage
declarations directly — Prisma, Drizzle, TypeORM, Sequelize/Mongoose,
Django, SQLAlchemy, pydantic/SQLModel, Rails/ActiveRecord, Eloquent, JPA,
GORM, Ecto, and raw `CREATE TABLE` from any source. The **structural layer**
sees code shape — classes, functions, routes — even when nothing is a data
model at all:

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

Coverage is reported **per repository, not as a global claim** —
`architect status`/`doctor`/`tour` all include exactly which languages and
frameworks were actually found in *this* codebase, so "why did this return
`ABSENT`" is never a mystery. A language with no extractor at all doesn't
get guessed at either: `INSUFFICIENT_COVERAGE` names the blind spot and
lists the files a human (or an AI, provisionally — see below) should
actually go read.

Deliberately not attempted anywhere: a real parser (everything here is
regex — an MVP tradeoff, not a hidden limitation) and any route DSL too
combinator-heavy to track reliably (Servant's type-level API, Akka HTTP's
in-code routing) — a wrong route is worse than a missing one.

## Clients: the same engine, three transports

| | Use | Command |
|---|---|---|
| CLI | scripting, terminal, CI | `architect <cmd>` |
| REST | GUI, dashboards, anything HTTP-shaped | `architect serve --port 7373` (127.0.0.1 only) |
| MCP | every AI coding tool | `architect mcp` (stdio) |

No business logic lives in any transport — REST and MCP are thin adapters
over the exact same query functions the CLI calls. Long-running server/
daemon processes (MCP, REST, `watch`) each detect if the binary on disk has
been rebuilt out from under them since they started, and surface a
`_stale_binary_warning` instead of silently answering from stale, in-memory
code — the fix in that case is simply restarting the process/session.

## Commands

```
architect init      --root DIR          build/refresh architect.db
architect status    --root DIR          what's declared, used, and structurally visible
architect concept   --root DIR TERM     does TERM exist? canonical? evidence?
architect intent    --root DIR "GOAL"   smallest correct change: EXTEND vs CREATE
architect plan      --root DIR "GOAL"   one-call composition of concept+owner+impact+decisions
architect impact    --root DIR TERM     what is affected if TERM changes
architect owner     --root DIR TERM     which directory owns TERM's declaration
architect guard     --root DIR "SQL"    THE LAW: rejects CREATE TABLE duplicating a concept
architect doctor    --root DIR          repository summary for someone who just cloned it
architect tour      --root DIR          onboarding: what matters, what's ignorable, past mistakes
architect duplicates --root DIR         suspected redundant concepts — risk, not proof
architect history   --root DIR [TERM]   the architectural timeline (what git can't say)
architect ci                            pipe a diff in, get an exit code out
architect laws                          the language specification, from laws/*.toml
architect watch     --root DIR          daemon: observe → notify, never act
architect serve     --port 7373         REST API (127.0.0.1, read-only except /proposal/*)
architect mcp                           MCP server over stdio
architect proposal  submit|list|inspect|test|accept|reject   the AI-extension protocol
```

## The proposal protocol: how an AI extends Architect

The only door through which a change — a new structural extractor, or a new
`architect.toml` decision/alias — can reach the repository, and it never
opens on its own. An AI proposes work, never evidence: `Tier::Inferred` does
not exist, and this protocol is the reason it doesn't need to.

```
architect proposal submit --kind extractor|decision|alias \
    --title "..." --patch some.diff      # inert patch, nothing applied yet
architect proposal test <id>             # applies it in an isolated git
                                          # worktree and runs it through the
                                          # SAME laws + invariants suite —
                                          # the real working tree is never
                                          # touched
architect proposal accept <id>           # only if: status == passed, the
                                          # patch is byte-identical to what
                                          # was tested, AND the repository
                                          # HEAD hasn't moved since — then
                                          # applies to the real working tree,
                                          # UNCOMMITTED. Architect never runs
                                          # `git commit`.
```

`check_scope()` hard-blocks any patch that touches the validation machinery
itself (`laws.rs`, `tests/laws.rs`, `tests/invariants.rs`, `store.rs`,
`model.rs`, `proposal.rs`, `Cargo.*`, `.github/`) or strays outside its
kind's allow-list — a proposal cannot weaken the suite it's judged by.
Reachable over the CLI, REST (`/proposal/*`), and MCP
(`proposal_submit`/`proposal_test`/...) — the same trust boundary regardless
of which client is holding the pen.

## The per-project ontology: architect.toml

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

The guard cites the governing decision when it rejects — "this table already
exists" states a fact; the decision states the reasoning, which is what stops
the same proposal returning next month under a different name.

## Memory model

This is a trust guarantee, not an implementation detail — projects never
share architectural memory, and no per-project state ever leaves that
project's own directory:

```
Architect core (compiled into the binary)
  └── laws/*.toml — universal rules about how the engine matches and ranks,
      the same for every project, never copied anywhere

Each project — <root>/architect.db (one SQLite file)
  ├── architecture state (concepts, structural graph)
  ├── decisions + aliases (mirrored from that project's own architect.toml)
  └── immutable event history (append-only)
```

A developer's `~/titan/architect.db` and another developer's
`~/payments/architect.db` are independent memories governed by the same
compiled-in laws. `init`/`save` only ever `INSERT OR REPLACE` known keys and
`CREATE TABLE IF NOT EXISTS` — re-running `init` (or the onboarding script)
against a project can never drop its history or decisions.

## Laws

14 active laws, each one a real wrong answer produced on a real repository,
each with a permanent regression fixture (`tests/fixtures/law_NNN/`) so it
can never silently recur. `conformance_registry_matches_suite` enforces the
pairing both directions — a law without a covering test fails CI, and a test
claiming a law the registry doesn't know fails too. `architect laws` prints
the full specification: statement, the wrong answer that taught it, and
which repository/session found it.

## Testing discipline

Two suites, different guarantees:

- `tests/laws.rs` — tiny synthetic fixtures, one per law: *the specific bug
  that taught us this rule cannot recur.*
- `tests/invariants.rs` — real cloned open-source repositories (chatwoot,
  lobe-chat, umami, Saleor, BookStack, dub, analytics, redash, Rails' and
  NestJS's own RealWorld ("Conduit") implementations for the schema layer;
  ASP.NET Core, C, Dart, Scala, Nuxt's own devtools monorepo, and gRPC's own
  canonical examples for structural-only checks — routes included, not just
  symbols): *the class of bug this invariant defines cannot occur in any
  scanned corpus, not just the one that found it.*

Every new language/framework extractor gets exercised against real code
before being trusted, not just a unit test — a synthetic snippet passing
was once not enough to catch an extractor that was completely unreachable
through the real scan pipeline, and a schema-layer real-repo check passing
was once not enough either (Rails' idiomatic `resources :articles` — a bare
Ruby symbol — was never matched by a route regex that only ever looked for
a quoted string, found the same way: by actually dogfooding a real
routes.rb, not by the unit test that already existed).

## Provenance

Born inside TITAN (an autonomous agent system) after a repeated failure:
"build X" → thirty minutes later → "X already existed." The fix was never
to remember to check; it was to make checking a capability, then a law, then
an always-running daemon. TITAN is now the first client.

## License

[Business Source License 1.1](LICENSE) — source-available, not permissive
open source. In plain language: free to read, run, modify, and use in
production — including at work, on your employer's codebases, as an
employee or paid contractor — for anything except turning Architect itself
into a competing commercial offering (reselling it as a hosted/managed
service, or bundling it into a paid developer-tools product) without a
separate commercial license. Converts automatically to Apache License 2.0
(fully open source) on 2030-09-01, or sooner if a future release sets an
earlier date.

For a commercial license, or any other licensing question: nevillejemo@gmail.com.
