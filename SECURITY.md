# Security

## Reporting a vulnerability

Email **nevillejemo@gmail.com** with what you found and how to reproduce
it. Please don't open a public GitHub issue for anything that isn't
already publicly exploitable — give a chance to fix it first.

There's no formal SLA (this is a small project), but a real report gets a
real response, not silence.

## What's actually in scope

Archietect runs entirely locally — there's no hosted service, no account
system, no data ever leaves the machine it runs on. The realistic attack
surface is:

- **`archietect serve`/`archietect gui`/the desktop app** — a REST server
  bound to `127.0.0.1` only (never `0.0.0.0`), so a remote attacker can't
  reach it directly. The remaining threat is a malicious page open in the
  *same browser* as the operator, firing a cross-origin request — GET
  requests need no CORS approval to be sent, only to have their response
  read. The five endpoints that write anything (`/proposal/{submit,test,
  accept,reject}`, `/system/register`) require `&token=...`, a random
  value printed once to stderr when the server starts and never persisted
  — nothing else on the machine can guess it. Every other endpoint is
  read-only by design.
- **`archietect mcp`** — stdio only, spawned by whatever AI tool is
  configured to run it. Trusts its own process's stdin the way any stdio
  MCP server does; the security boundary is "which AI tools you've
  configured to run this," not something this server can enforce itself.
- **The permissions system** (`src/permissions.rs`) — a hardcoded denial
  list (`.ssh`, `.aws`, credential/secret filenames, browser profiles) is
  checked *first*, unconditionally, before any project or global config is
  even consulted. No `archietect.toml`, no `~/.archietect/system.toml`,
  can re-enable a hardcoded denial. If you find a path that should be
  denied and isn't, that's a real, in-scope report.
- **The unstructured domains** (`documents`/`photos`/`messages`/`docker`)
  — each is opt-in per project (`[domains.X]` must be explicitly set) and,
  where the CLI would normally prompt for one-time confirmation, MCP/REST
  callers get a hard `enabled: false` instead of a guessed "yes" — there's
  no transport-level way to answer a prompt that was never asked. Content
  is never read for documents/photos/messages — filename, extension,
  size, and mtime only.

## What's explicitly NOT a vulnerability report

- "The REST server has no authentication on read endpoints" — intentional;
  see rest.rs's own module doc. It binds to loopback only and every GET is
  read-only.
- "Anyone with local file access to the machine can read `archietect.db`"
  — true of every local dev tool that persists state to disk; not a threat
  this project's model is trying to defend against.
- Findings in the `laws/` or `tests/fixtures/` synthetic corpus — those are
  deliberately-constructed examples of bug *shapes*, not real
  vulnerabilities in the engine.
