<!-- archietect:agent-instructions:begin -->
## Before you create something new

This project has [archietect](https://github.com/Neville777/Archietect)
attached — a deterministic index of what already exists here (concepts,
schema, routes, decisions), kept warm by a background daemon.

Before creating a new table, model, endpoint, or class — or before telling
someone something does not exist in this codebase — check first:

    archietect concept <name>       # does it exist? canonical? evidence?
    archietect intent "<goal>"      # smallest correct change: EXTEND vs CREATE
    archietect impact <name>        # what breaks if you change it
    archietect duplicates           # suspected redundant concepts already here

If archietect is registered as an MCP server in this environment, call
these as tools instead of shelling out. A commit that introduces a
duplicate concept may also be rejected automatically by a pre-commit hook —
see `archietect ci` if that happens.
<!-- archietect:agent-instructions:end -->
