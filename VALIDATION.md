# Validation ledger — every wrong answer becomes a law

> The machine-readable registry is `src/laws.rs` (`architect laws`); the
> enforcement arm is `tests/laws.rs` — one self-contained fixture per law,
> run by `cargo test`. This file is the narrative; those are the spec.
> A law without its regression test is a wish.

The maturation loop: run against real repositories, record every wrong
answer, turn each into a rule the engine enforces forever. No wrong answer
is fixed by patching the output; it is fixed by changing what the engine is
allowed to believe.

## Corpus so far
31 local projects + 6 public OSS repos (umami, dub, full-stack-fastapi-template,
saleor, redash, excalidraw) + TITAN (~200 concepts, Rust/raw-SQL).

## Laws, in the order the corpus taught them

1. **Word-boundary matching, never substring.** `%sd%` matched USD and
   declared a broken pipeline healthy; `story` claimed `harvest_hiSTORY`.
2. **The guard's re-declaration exemption is EXACT-name only.** Fuzzy
   matching allowed `CREATE TABLE ghosts` beside model `Ghost`. Near-names
   are what duplicates look like, so near-names must block.
3. **Prose about schema is not schema (comments).** The guard's own doc
   comment — "a patch proposing CREATE TABLE episodes is REJECTED" — minted
   a phantom `episodes` concept that then satisfied the exemption and
   defeated the guard. Comment lines are stripped before SQL extraction.
4. **Exact-name match outranks token match.** umami: `website` returned
   WebsiteEvent (more usage) while a model literally named Website existed.
   The thing named for the concept IS the concept.
5. **Declarations sharing a table are ONE concept.** umami declared
   `website` (SQL migration) and `Website` (Prisma @@map) — one concept,
   reported as competing with itself. SQL-only declarations fold into the
   ORM concept mapped to the same table.
6. **`table=True` declares storage regardless of base names.**
   fastapi-template: `class Item(ItemBase, table=True)` names neither
   SQLModel nor BaseModel; the real storage models were invisible while
   their API contracts were indexed.
7. **ORM declarations outrank SQL-string-only concepts on exact ties.**
   redash: a phantom lowercase `query` outranked the real Query model on
   usage inflated by every `FROM query` in the repo.
8. **Prose about schema is not schema (strings).** The `query` phantom came
   from `logger.debug("CREATE TABLE query: %s")` — a log message. A real
   CREATE TABLE is followed by `(` or AS; requiring the follower kills
   log/prose strings structurally, with no blacklist.

## Extractors added because the corpus demanded them
pydantic (Sentinel: models.py with zero Django), SQLModel (fastapi-template),
SQLAlchemy (redash: 4 → 23 concepts), CREATE TABLE from ALL sources
(TITAN: 110 DDLs in Rust string literals; 92 → 198 concepts).

## Current scorecard (all verified, this corpus)
umami website→Website · dub link→Link (+ guard blocks `links`) ·
saleor order→Order, checkout→Checkout · redash query→Query(queries),
dashboard→Dashboard(dashboards) · template item→Item(item) ·
TITAN guard episodes→BLOCKED citing ADR · ghosttrack guard ghosts→BLOCKED ·
excalidraw→0 concepts (honest: no schema in repo).

## Known open weaknesses (documented, not hidden)
- Usage detection misses GraphQL resolvers and raw drivers → live concepts
  can read DECLARED_ONLY (hence its "confirm" wording).
- Django table names not derived (app-label needed) — None over a guess.
- Mongoose, Rails, Go (gorm/ent), Java (JPA) extractors absent.
- Canonical ranking is a heuristic validated on this corpus, not a proof.
