<!-- archietect:agent-instructions:begin -->
## You are working on Archietect itself. Read this before touching anything.

This repository has three hard enforcement layers that will stop you if you
skip them. They are not suggestions.

### Layer 1 — Pre-tool-use hook (fires on every file write)
The `.claude/hooks/archietect-guard.sh` hook intercepts every `Write` call.
If you try to create a file whose name resolves to a concept that already
exists in the index, the write is **blocked** with exit code 2. You cannot
proceed without acknowledging the existing concept.

### Layer 2 — Pre-commit hook (fires on every commit)
`.git/hooks/pre-commit` pipes the staged diff through `archietect ci`.
If the diff introduces a duplicate concept or violates an architectural law,
the commit is **rejected**. This catches anything that slipped past Layer 1.

### Layer 3 — CI gate (fires on every push/PR)
`.github/workflows/ci.yml` runs `archietect ci` on the diff. A PR that
introduces a duplicate or law violation will **fail CI** and cannot be merged.

---

### What you must do before creating anything new

```bash
archietect concept <name>       # does it exist? where? what's the evidence?
archietect intent "<goal>"      # EXTEND an existing concept or CREATE new?
archietect impact <name>        # what breaks if you change it?
archietect duplicates           # anything suspiciously similar already here?
```

If archietect is registered as an MCP server in this environment, call these
as MCP tools — they are faster and don't require a shell. The MCP server is
started with `archietect mcp` (stdio transport).

### Key concepts in this codebase (from `archietect doctor`)
- **Index** (121 uses) — the core data structure. Do not create a second one.
- **Symbol** (62 uses) — the structural extraction unit. Extend, don't duplicate.
- **Concept** — schema-layer concept. Different from Symbol. Do not conflate them.
- **ObservationSource::Ast** vs **ObservationSource::Lexical** — the distinction
  introduced in 0.1.12. New extractors must set the correct source.

### Laws you cannot violate (from `archietect laws`)
- **law-015**: Never return a confident ABSENT when coverage is insufficient.
- **law-014**: A file in a language that has an extractor must be walked.
- Any new extractor must pass `cargo test --test laws` before commit.

### The proposal protocol
AI agents do not commit directly to `laws/`. If you believe a law needs
changing, submit a proposal:
```bash
archietect proposal submit --kind decision --title "..." --patch some.diff
archietect proposal test <id>
archietect proposal accept <id>   # only after: status==passed, diff unchanged
```
<!-- archietect:agent-instructions:end -->
