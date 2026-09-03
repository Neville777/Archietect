#!/usr/bin/env python3
"""Drives AME's REAL MCP tool functions in-process (same code the MCP stdio
server calls in memory_engine/mcp/server.py -- verified by reading that file
and tests/test_phase13_mcp.py). Bypasses only the JSON-RPC envelope, not any
business logic. Never touches archietect's repo; never modifies AME's repo.

Usage:
  python ame_driver.py bootstrap <project_root>
  python ame_driver.py status <project_root>
  python ame_driver.py retrieve <project_root> <task> [task_intent] [--files f1,f2] [--symbols s1,s2]
"""
import sys, json, time, argparse
from pathlib import Path

sys.path.insert(0, "/tmp/agent-memory-engine")

from memory_engine.mcp.project_context import ProjectContext
from memory_engine.mcp.schemas import RetrieveContextInput
from memory_engine.mcp.tools import (
    tool_memory_status,
    tool_refresh_project_knowledge,
    tool_retrieve_agent_context,
)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["bootstrap", "status", "retrieve"])
    ap.add_argument("project_root")
    ap.add_argument("task", nargs="?", default=None)
    ap.add_argument("task_intent", nargs="?", default="unknown")
    ap.add_argument("--files", default="")
    ap.add_argument("--symbols", default="")
    ap.add_argument("--budget", type=int, default=6000)
    args = ap.parse_args()

    ctx = ProjectContext(Path(args.project_root))

    if args.cmd == "bootstrap":
        t0 = time.time()
        report = ctx.ensure_bootstrapped()
        refresh = tool_refresh_project_knowledge(ctx)
        status = tool_memory_status(ctx)
        t1 = time.time()
        print(json.dumps({
            "bootstrap_report": report,
            "refresh": refresh,
            "status": status,
            "elapsed_s": round(t1 - t0, 2),
        }, indent=2, default=str))
        return

    if args.cmd == "status":
        print(json.dumps(tool_memory_status(ctx), indent=2, default=str))
        return

    if args.cmd == "retrieve":
        files = [f for f in args.files.split(",") if f]
        symbols = [s for s in args.symbols.split(",") if s]
        inp = RetrieveContextInput(
            task=args.task,
            task_intent=args.task_intent,
            current_files=files,
            current_symbols=symbols,
            token_budget=args.budget,
        )
        t0 = time.time()
        out = tool_retrieve_agent_context(ctx, inp)
        t1 = time.time()
        out["_elapsed_s"] = round(t1 - t0, 3)
        print(json.dumps(out, indent=2, default=str))
        return


if __name__ == "__main__":
    main()
