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
          Architect (MCP / CLI)
                │
        Architectural state              ← truth lives HERE
   laws · concepts · decisions · aliases
   evidence · provenance · history
                │
    source code · schemas · ADRs
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
   guard rejects; it never rewrites.
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

## Commands

```
architect init    --root DIR            build/refresh architect.db
architect status  --root DIR            what the index knows (and admits it can't see)
architect concept --root DIR TERM       does TERM exist? what is canonical? evidence?
architect intent  --root DIR "GOAL"     smallest correct change: EXTEND vs CREATE
architect impact  --root DIR TERM       what is affected if TERM changes
architect guard   --root DIR "SQL"      THE LAW: rejects CREATE TABLE that duplicates a concept
architect why?    → declared in architect.toml decisions; guard cites them on rejection
architect laws                          the language specification, from laws/*.toml
architect watch   --root DIR [--subscribe C]   daemon: observe → notify, never act
architect history --root DIR [CONCEPT]  the architectural timeline (what Git can't say)
architect mcp     [--root DIR]          MCP server: every AI tool becomes a client
```

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

## Extractors (v0)

Prisma · Django · pydantic · SQLModel (`table=True` is storage) · SQLAlchemy
· raw SQL (`CREATE TABLE` from ALL sources — comment-stripped, `(`-follower
required, because prose about schema is not schema). Everything unparsed
degrades honestly to NAMED, never silently.

## Provenance

Born inside TITAN (an autonomous agent system) after a repeated failure:
"build X" → thirty minutes later → "X already existed." The fix was never
to remember to check; it was to make checking a capability, then a law, then
an always-running daemon. TITAN is now the first client.
