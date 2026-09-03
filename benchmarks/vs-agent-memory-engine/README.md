# archietect vs. Agent Memory Engine

A real, reproducible comparison — not an assertion. Every number below comes
from a script in this directory, run against real open-source repositories
already in this repo's `validation/` corpus. Nothing here is self-reported
by either tool; both were queried the same way and scored by the same
criteria.

**Comparable against**: [Agent Memory Engine](https://github.com/uudam42/agent-memory-engine)
(AME) — chosen because it's the most architecturally similar tool to
archietect: local-first, SQLite-backed, MCP server, no required cloud
account. Its default retrieval mode (lexical/FTS5, no semantic embeddings
installed) is what was tested — see **Limitations** below.

## What was measured

**Accuracy** — for 21 terms across 3 repos (`golang-gin-realworld-example-app`,
`spring-petclinic`, `nestjs-realworld-example-app`), each with a ground-truth
answer established by hand (`queries.json`), both tools were asked the same
question and scored on whether they surfaced the term's real declaring file.

**Speed** — wall-clock time per query, same 21 queries.

## How to reproduce

```bash
# from the archietect repo root — validation/ is gitignored (real third-party
# repos, not vendored), so clone the 3 this benchmark uses first:
git clone https://github.com/gothinkster/golang-gin-realworld-example-app validation/golang-gin-realworld-example-app
git clone https://github.com/spring-projects/spring-petclinic validation/spring-petclinic
git clone https://github.com/lujakob/nestjs-realworld-example-app validation/nestjs-realworld-example-app

cargo build --release
target/release/archietect init --root validation/golang-gin-realworld-example-app
target/release/archietect init --root validation/spring-petclinic
target/release/archietect init --root validation/nestjs-realworld-example-app

git clone https://github.com/uudam42/agent-memory-engine.git /tmp/agent-memory-engine
cd /tmp/agent-memory-engine && bash scripts/install.sh && cd -

python3 benchmarks/vs-agent-memory-engine/run_benchmark1.py   # writes result_*.json
python3 benchmarks/vs-agent-memory-engine/score.py             # prints the scorecard
```

## Results

For the 15 of 21 queries with a single, unambiguous declaring file:

| | archietect | AME (lexical mode) |
|---|---|---|
| Correct declaring file surfaced | **15/15** | **5/15** |
| Gives an exact line number | Yes, for structural symbols | No — whole-file chunks only |
| Signals "this doesn't exist" | Yes — `ABSENT`/`INSUFFICIENT_COVERAGE` | No — always returns ranked chunks, even for absent terms |
| Mean time per query | ~14ms (full process spawn) | ~120ms (in-process call — real MCP stdio overhead would add more) |

Raw per-query results, including the 6 `AMBIGUOUS`/`ABSENT` queries not in
the table above, are in `result_*.json`.

## Limitations — read before citing these numbers

- **AME's semantic/embedding retrieval mode was never tested.** Only its
  default lexical-only mode (no `sentence-transformers`/`sqlite-vec`
  installed) was benchmarked. AME's own docs suggest semantic mode changes
  retrieval behavior meaningfully; that comparison has not been run.
- **N=21 queries across 3 small-to-medium repos.** This is not a claim of
  universal superiority — it's what was actually measured, on these repos,
  with these queries.
- **Speed numbers call AME's underlying Python function in-process**, not
  through the real MCP stdio/JSON-RPC transport — a real client would see
  higher AME latency than reported here, not lower.
- A second test (duplication-prevention, on a real production codebase with
  known fragmented services) was also run and produced results consistent
  with the table above, but its raw data is not published here — that
  codebase is a private, third-party project, not part of this repo's
  validation corpus.
