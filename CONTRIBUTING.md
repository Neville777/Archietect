# Contributing

There are two separate doors into this codebase, for two different kinds of change.

## Human PRs — code, bugs, features

Normal GitHub flow: fork, branch, PR against `main`. Nothing exotic.

**Before opening one:**

```bash
git clone git@github.com:Neville777/Archietect.git archietect && cd archietect
cargo build --release
cargo test --test laws --verbose     # tests/laws.rs — fast, self-contained
cargo test --lib --verbose           # unit tests embedded in src/
```

That's exactly what `.github/workflows/ci.yml` runs on every push and PR
(plus `tests/invariants.rs`, which only runs if a `validation/` corpus of
cloned real-world repos happens to be present — it's `.gitignore`d and
never committed, so it's a no-op on a fresh clone; nothing you need to set
up to get a green CI run). `cargo fmt --check` and `cargo clippy` are
**not** part of the required gate yet — the workflow's own comment
explains why (pre-existing diffs/warnings nobody's cleaned up yet, not a
statement that style doesn't matter). Match the surrounding file's
formatting by eye in the meantime.

**What this codebase actually expects from a PR**, beyond compiling and
passing tests — read a few existing files (`src/rest.rs`, `src/scan.rs`,
`src/query.rs` are dense examples) before writing new code, and match the
pattern:

- **Comments explain WHY, not WHAT.** A comment that restates the code
  below it in English gets deleted on sight in review. A comment earns its
  place by naming a constraint, a bug it prevents, or a real incident that
  shaped the decision — "found live: X did Y, which broke Z" is the
  standard, not "// increment counter."
- **No invented facts.** This project's entire premise is "an AI reasons on
  top of deterministic, evidenced state instead of guessing." Code that
  asserts something without a real, checkable basis for it — a fabricated
  test fixture that doesn't reproduce an actual bug shape, a claim in a
  comment that isn't actually true of the code near it — undermines that
  premise more than a missing feature would. If you found a real bug on a
  real repo, say so specifically (what broke, on what input); don't
  generalize past what you actually verified.
- **Never name another project, company, or person's codebase in code,
  comments, or test fixtures** — including your own employer's, even
  anonymized-but-recognizable details (a distinctive crate/table/module
  name lifted from a real private repo). Earlier history in this repo
  leaked exactly this way and had to be redacted after the fact; describe
  the bug SHAPE generically ("a table and an unrelated struct shared one
  generic token") instead.
- **A new test is a regression test for something real**, not a coverage
  target. `tests/laws.rs` ties one synthetic fixture to one specific bug
  this engine previously produced — that's the bar.
- Prefer fixing the actual root cause over adding a special case. This
  project's own history (see recent commit messages) has examples of a
  first attempt landing as a narrow patch and getting replaced by the
  general fix once the pattern repeated — better to spot that up front.

**Off-limits without a maintainer, even in a normal PR:**

- `laws/` — a law is a policy decision about what Archietect is allowed to
  ever assert; only a maintainer mints one, in a real release. This mirrors
  `check_scope()`'s own hard block on the AI proposal protocol below —
  the same restriction, for the same reason, applies to a human PR too.
- The validation machinery itself (`src/laws.rs`, `tests/laws.rs`,
  `tests/invariants.rs`, `src/store.rs`, `src/model.rs`, `src/proposal.rs`,
  `Cargo.*`, `.github/`) — changes here need a maintainer's judgment call,
  not a drive-by PR, since they're what everything else is judged against.

Found a genuine defect and don't want to fix it yourself? File it:
https://github.com/Neville777/Archietect/issues — no template required,
just what broke and on what input.

## AI-authored changes — the proposal protocol

A structural extractor for a new language, or a new `archietect.toml`
decision/alias, proposed by an AI agent rather than typed by a human: see
the README's own "Contributing: the proposal protocol" section. Different
mechanism (`archietect proposal submit/test/accept`, gated by real test
runs in an isolated worktree, human-gated at the final `accept` step) for
a narrower, machine-mediated class of change — not a general PR
replacement, and it can't touch `laws/` either.

## License

By submitting a PR, you agree your contribution is licensed under the same
terms as the rest of the project — see [LICENSE](LICENSE) (Business Source
License 1.1). Licensing questions: nevillejemo@gmail.com.
