# System memory: from project index to world model

Status: **design draft, not implemented.** Nothing in this document describes
current behavior. It exists so the discussion that produced it doesn't
evaporate, and so implementation has a spec to build against instead of
starting from a slogan.

## The vision

A system has reality, but no persistent, trustworthy memory of that reality.
What exists, what depends on what, what is running, what was declared, what
was observed, and what changed are continuously rediscovered — by AI, by
developers, by CI, by monitoring, by whoever happens to be asking — and each
of those reconstructions is temporary, private, and unverifiable the moment
it's spoken. Nothing accumulates. Everyone independently re-derives the same
reality, forever, and none of it becomes the system's own memory of itself.

Archietect is that memory: an evidence-backed, provenance-preserving record
of what exists and how things relate, with explicit boundaries around what
it's allowed to observe, and an explicit distinction between what is
declared, observed, derived, and unknown. It does not decide what is true —
it records what can be established, and where that knowledge came from.

**AI is a client of Archietect, not its purpose.** So is a human, a CI job,
a migration tool, a monitoring service, or any other program that would
otherwise have to rediscover the same fact independently. AI is currently
the client paying the steepest rediscovery tax, which is why it's the
loudest use case — but the problem this solves predates AI and isn't
bounded by it. The goal is not better search. The goal is for a system to
have memory of itself, so search becomes the fallback for what's genuinely
unknown (`INSUFFICIENT_COVERAGE`) instead of the default way anything gets
known at all — and once that fallback discovery happens, its result can
become part of the persistent memory instead of evaporating again.

The test for every feature from here is: **does this make the system's
persistent memory of reality more complete, more trustworthy, or more
accessible?** If not, it doesn't belong here, regardless of how useful it
would otherwise be.

Archietect today answers "does this concept already exist in this codebase."
The rest of this document is about generalizing that beyond code — the
premise being that the codebase was never the point, it was the domain
where the underlying idea was easiest to prove, because code is the
cleanest artifact class that formally declares itself. Everything below is
what changes, and what deliberately doesn't, to get there.

## What doesn't change

- **Archietect does not use AI. AI uses Archietect** — one memory, many
  consumers, none of them privileged. This gets more important at world
  scale, not less — the bigger the memory, the more it matters that the
  thing holding it never guesses.
- **Evidence over inference.** Every fact still needs a reason it's believed,
  tiered by strength.
- **Absence of evidence is disclosed, not hidden.** `INSUFFICIENT_COVERAGE`
  stays a first-class answer, not a fallback to a guess.
- **Laws protect the architecture; they aren't the architecture.** See
  below — this needed a correction during design, not a rewrite.

## Core abstraction: Resource

Every domain-specific extractor today (Rust, Python, Prisma, gRPC, ...)
already produces the same shape: a name, a location, an evidence tier, some
attributes. That shape generalizes without modification:

```
Resource {
    id:         Identity,     // see Identity, below — not just a name
    kind:       String,       // "rust_struct", "systemd_unit", "docker_container", ...
    domain:     String,       // "code", "process", "container", "service", "document", ...
    location:   Location,     // file:line | "systemd:user:foo.service" | "pid:1234" | ...
    attributes: Map,          // domain-specific key/values
    evidence:   Evidence,     // see below
}
```

The core engine (matching, ranking, `duplicates`, `concept`) operates only on
this shape. It does not know what a "systemd unit" is, the same way it
doesn't currently know what a "Prisma model" is beyond the extractor that
produced it. Adding a domain is adding an extractor, not teaching the core a
new concept.

## Evidence has two vocabularies, not one

The DECLARED > USED > NAMED ordering works because code and infrastructure
are *formal* — a schema, a systemd unit file, a Dockerfile explicitly asserts
what it is. That stops being true once the domain is personal/unstructured
content (photos, notes, messages, browser history). Forcing those through
the same three words either silently weakens what DECLARED means, or invites
exactly the failure mode this project exists to prevent: an AI's content
guess dressed up as an observed fact.

**Structured domains** (code, infra, services, packages, git):

```
DECLARED   an explicit, formal source asserts this (schema, unit file, manifest)
USED       code/config observably references it
OBSERVED   it is verified live right now (running process, open socket)
NAMED      only a name/naming convention suggests it — weakest, never
           sufficient alone for a confident answer
```

**Unstructured domains** (documents, media, messages, notes):

```
EXPLICIT   the user directly labeled/tagged it
DERIVED    structured metadata says so (EXIF, filename convention, calendar
           field) — not content interpretation
INFERRED   content analysis suggests it — visibly and permanently the
           weakest tier; INFERRED must never be presentable as DECLARED
```

A verdict must always say which vocabulary produced it. `INFERRED` from the
unstructured vocabulary is not the same claim as `NAMED` from the structured
one, and the two must never be merged into one undifferentiated "weak"
bucket — that merge is exactly where confident-sounding wrong answers come
from.

## Every fact has an as-of time, not just a tier

DECLARED evidence from a source file is stable until someone edits it.
OBSERVED evidence ("Redis is running") can become false a second after it's
recorded. Once OBSERVED is a real tier — not a scan-time snapshot, a live
claim — every fact needs a timestamp and, for OBSERVED specifically, an
explicit staleness/TTL, or the engine can report a process that died ten
minutes ago with the same confidence as one that's alive right now. This is
a fourth property alongside tier and provenance, not a variant of either.

## Relationships need their own evidence, not borrowed evidence

"GhostTrack depends_on Redis" is not implied by GhostTrack existing and
Redis existing. The edge is a fact with its own provenance:

```
Relationship {
    from:      ResourceId,
    kind:      String,        // "depends_on", "declares", "runs_as", "watches", ...
    to:        ResourceId,
    evidence:  Evidence,      // tiered exactly like a resource's evidence
}
```

Example: `depends_on` can be DECLARED (a docker-compose.yml names both) or
OBSERVED (an actual open connection between the two processes) — those are
different claims and must be presented as such. Skipping edge-level evidence
turns the graph into "these two things look related," which is precisely the
plausible-but-unverified memory this project is supposed to prevent.

## Identity is a link, and the mechanism for it already exists

Two resources being "the same thing" cannot be inferred from a shared name —
`redis` the Docker container, `redis` the systemd service, and `redis` the
word in a README are three different claims. This isn't a new primitive to
invent: `architect.toml`'s `[aliases]` section (`episode = "stories"`) is
already exactly this — a declared identity link with its own provenance (the
toml file, mirrored into the db, cited by the guard when it rejects). The
world-scale version generalizes *who is allowed to assert a link*, not the
mechanism itself:

- **Human-declared** — an alias/decision, same as today.
- **Extractor-observed** — a compose file naming both a container and a
  service; a systemd `ExecStart` pointing at a binary built from a known
  repo. Real, checkable, structural evidence of shared identity.
- **Name similarity** — never sufficient alone. It's a candidate for a human
  or a proposal to confirm, exactly like `archietect duplicates` today
  reports "risk, not proof."

## Two-level graph, not one flattened store

The Memory Model's existing trust guarantee — "no per-project state ever
leaves that project's own directory" — has to survive this. A single
`~/.archietect/system.db` holding *every fact from every project* would
break it. Instead:

```
~/.archietect/system.db      — cross-domain resources, relationships,
                                and POINTERS into project dbs (not copies)
<project>/archietect.db      — remains the physical source of truth for
                                that project's own facts, exactly as today
```

The system graph can answer "which projects use Redis" by following
pointers and querying the relevant project db live, the same way `--root`
resolution works today — it doesn't require centralizing project state to
be globally queryable.

## Memory boundaries are the default, not an add-on

There is no "scan everything" mode. Every domain is off until explicitly
enabled. This section is the finalized design (decided, not just sketched)
for phase 5's prerequisite — reusing two mechanisms this codebase already
has rather than inventing new ones: the global/per-project config split
(MCP registration is global; `archietect.toml` is per-project) and the
hardcoded, non-overridable blocklist pattern (`proposal.rs`'s
`FORBIDDEN_EXACT`/`FORBIDDEN_PREFIX`).

### Config surface — two layers

```toml
# ~/.archietect/system.toml — global defaults, every project inherits these
[domains]
code = "enabled"     # the tool's original contract — never was opt-in
git  = "enabled"     # phase 3's domain — read-only, structural, low sensitivity
# every other domain: absent = disabled. Default-deny, not default-allow.
```

```toml
# <project>/archietect.toml — per-project override. Reuses the SAME file
# phase-0 already uses for [aliases]/[[decision]] — one config file per
# project, not a second one with its own onboarding/documentation surface.
[domains]
docker = "enabled"   # this project only
```

### Precedence, strict-descending, no exceptions

1. **Hardcoded denial list** — not configurable by any toml, project or
   global: `~/.ssh`, `~/.aws`, `~/.gnupg`, browser profile directories,
   anything matching a credentials/secrets naming pattern. Same shape as
   `proposal.rs`'s forbidden-path constants — a proposal cannot weaken the
   suite it's judged by; a config file cannot re-enable a hardcoded denial.
2. **Project-level `archietect.toml` `[domains]`** — overrides global, for
   that project only.
3. **Global `~/.archietect/system.toml` `[domains]`** — the default for
   everything else.
4. **No entry for a domain, in either file** = disabled. A domain that
   hasn't shipped yet, or that nobody has configured, is never silently on.

### Attribute scoping (relevant once unstructured domains exist)

```toml
[domains.photos]
state = "enabled"
attributes = ["filename", "metadata"]   # explicitly NOT "content" —
                                         # no vision/content analysis
                                         # without a separate, louder opt-in
```

Code and git don't need this — they already operate at content-level by
nature (a symbol declaration IS the content); attribute scoping only
becomes meaningful for domains where "look at the filename" and "analyze
the content" are genuinely different acts with different privacy weight.

### Enable friction: structured vs. unstructured domains

Editing `[domains]` in either toml file is suffient, on its own, to enable
a **structured** domain (code, git, docker, systemd, package manifests) —
these are already low-sensitivity and this project already asks a user to
run `onboard.sh` deliberately once per project. But enabling ANY
**unstructured** domain (photos, messages, documents, browser) additionally
requires a one-time interactive `y/N` confirmation via the CLI at first use
— the same shape as `onboard.sh`'s existing daemon-install prompt — so it
can never take effect purely from a config file someone else wrote, an
agent edited unsupervised, or a copy-pasted `system.toml`. A `--non-interactive`
context (CI, a script) that hits an unstructured domain awaiting confirmation
fails closed (treats it as disabled) rather than silently assuming consent —
same failure direction `onboard.sh --non-interactive` already takes for the
daemon prompt.

### Enforcement

One gate, called by every extractor before it does any real work — matching
the `Extractor` trait sketch below:

```rust
permissions::domain_allowed(cfg, "docker") -> bool
permissions::resource_allowed(cfg, "docker", &path) -> bool  // hardcoded
                                                              // denial list
                                                              // checked
                                                              // here first,
                                                              // unconditionally,
                                                              // before any
                                                              // config is
                                                              // even read
```

`git_domain::scan()` (phase 3) currently runs unconditionally with no gate
at all — implementing this permission model must retrofit that call site,
not just apply to domains added after it.

"Archietect knows my machine" has to mean "it has a declared boundary and
everything inside it has provenance," never "it indexed everything by
default." This is not a caveat bolted onto the design — it is the
architectural difference between this and the class of product (blanket
device-activity indexing, shipped as a default-on feature) that has already
been a public security/privacy failure elsewhere.

## Instruction files vs. memory vs. enforcement — three things, not one

`CLAUDE.md`, `AGENTS.md`, READMEs and architecture docs are easy to mistake
for what Archietect is, and the difference matters most exactly when an AI
is about to act.

| Thing | What it is | Can the consumer ignore it? |
|---|---|---|
| `CLAUDE.md` / `AGENTS.md` | Instructions — what a human *wants* the AI to do | Yes. It can skip the file, misread it, forget it mid-task, or decide a different file is "safe". |
| Documentation | A human-authored *description* of the system, true when written | Yes — and it has to be found, read, judged relevant, judged still true, and reconciled with the real repository first. |
| Archietect | A machine-maintained *memory* of the system: identity, evidence, provenance, relationships, as-of time, and an explicit boundary of what it may observe | The memory: no more than it can ignore `git`. The boundary: **no** — when wired into the consumer's execution path, denial is a property of the infrastructure, not a request. |

Documentation says "GhostTrack uses Redis." Archietect says: GhostTrack
`depends_on` Redis, declared in `docker-compose.yml:42`, Redis container
observed at 2026-09-03T…, and — separately — whether it is *running* is
not established by any extractor here. The first is a sentence somebody
wrote. The second is a record with receipts and an honest edge.

Two consequences, both already partly real in this codebase:

1. **Register-first, not remember-to-read.** An instruction file only works
   if the AI reads it. The register (`archietect register`, the "what is
   known / not known / allowed" map) can instead be handed to the AI at
   session start by the tooling itself, so asking the memory becomes the
   default first move rather than a habit it has to keep.
2. **The boundary is enforced where the AI acts, not described where it
   reads.** `permissions.rs` already fails closed, never reads content it
   wasn't allowed to, and cannot be re-enabled past the hardcoded denials
   by any config. Wiring that same boundary into the AI's tool calls (a
   pre-tool hook consulting `archietect permissions` before a `Read`/`Edit`/
   `Write` lands) turns "please don't touch `production.env`" into an
   operation that is rejected. The AI does not get a vote.

This is why the integration deliberately narrows to **Claude Code** rather
than staying vendor-generic: advisory text can be generic precisely because
it is ignorable; enforcement has to hook into one tool's real execution
path, and Claude Code's hooks are that path. `CLAUDE.md` still tells the AI
what you want. Archietect tells it what the world establishes — and, where
hooked in, what it is structurally allowed to do. Neither replaces the
other; they answer different questions.

## Where the laws model actually stands (correction from design discussion)

The original worry was "will N users create N×laws." They won't, and the
existing code already prevents it:

| Layer | Shared across users? | Grows with usage? |
|---|---|---|
| Core laws (`laws/*.toml`) | Yes | Slowly — only for a genuinely new invariant |
| Conformance fixtures (`tests/laws.rs`, `tests/invariants.rs`) | Yes | Yes — new fixture per new instance of an existing invariant |
| Extractors/adapters | Yes | Yes — one per new domain |
| Memory/evidence/graph | No — per project (and, in this design, per machine) | Continuously |

`law-015` ("must not return confident ABSENT under insufficient coverage")
is already an engine invariant, not "the Django bug." A thousand users
hitting insufficient-coverage bugs in a thousand different frameworks adds a
thousand *fixtures* under a small, stable set of laws — never a thousand new
laws. `conformance_registry_matches_suite` already enforces this shape. The
laws model does not need to change for this design; only the fixture corpus
and the extractor count grow.

## Extractor trait (sketch — not final)

```rust
trait Extractor {
    fn domain(&self) -> &'static str;
    fn detect(&self, scope: &Scope) -> bool;       // is this domain present/enabled here?
    fn scan(&self, scope: &Scope) -> Vec<Resource>;
    fn relationships(&self, resources: &[Resource]) -> Vec<Relationship>;
}
```

Same shape as the existing per-language structural extractors and the
"Structural coverage" reporting already in the README — adding a domain is
implementing this trait and registering it, not modifying the core.

## What's actually hard here (design budget, in priority order)

1. **Identity** — when are two observations the same entity. (Generalizing
   the alias mechanism; deciding what counts as sufficient link-evidence per
   domain pair.)
2. **Edge provenance** — what evidence justifies a relationship, and at what
   tier.
3. **Memory boundaries** — the permission model above, correctly scoped
   *before* any domain beyond code ships.

Everything else — Docker, systemd, package managers, documents, photos — is
an adapter/observation problem once those three are right, not a rewrite of
the engine.

## Non-goals

- Archietect does not become an AI. It never ranks by plausibility or
  content-similarity as a substitute for evidence.
- No domain is ever indexed by default. Ever.
- The per-project trust guarantee for code does not get weaker to make the
  system graph possible.

## Suggested phased rollout (not committed, for discussion)

1. Internal-only: generalize `Concept` toward `Resource` in the existing
   codebase, zero behavior change, so the code domain is provably just one
   instance of the new shape.
2. Add the `Relationship`/edge-evidence type, still code-only (e.g. "route
   calls concept" becomes a real evidenced edge instead of implicit).
3. First non-code domain as a proof of the generalization: something formal
   and low-risk, e.g. `git` (repos, remotes, branches) or `systemd`/`launchd`
   units — both already touched by `packaging/`.
4. System-level db + pointer model, still single-domain-shallow.
5. Only after 1–4 are real: design the unstructured-domain evidence
   vocabulary and the first personal-content domain, with the permission
   model as a hard prerequisite, not a follow-up.
