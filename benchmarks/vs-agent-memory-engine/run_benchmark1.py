#!/usr/bin/env python3
"""Run benchmark 1 (accuracy) + benchmark 3 (speed) across 3 repos.
Calls archietect binary via subprocess, and AME's real tool functions
in-process (same functions memory_engine/mcp/server.py wraps for MCP/stdio).
Writes one result JSON per repo into this directory.

Prerequisites:
  - Run from anywhere, with `cargo build --release` already done at the
    archietect repo root, and `validation/<repo>` already `archietect
    init`-ed.
  - Agent Memory Engine cloned at /tmp/agent-memory-engine with `uv sync`
    run there — see https://github.com/uudam42/agent-memory-engine.
"""
import sys, json, subprocess, time
from pathlib import Path

sys.path.insert(0, "/tmp/agent-memory-engine")
from memory_engine.mcp.project_context import ProjectContext
from memory_engine.mcp.schemas import RetrieveContextInput
from memory_engine.mcp.tools import tool_retrieve_agent_context

REPO_ROOT = Path(__file__).resolve().parents[2]
ARCH = str(REPO_ROOT / "target" / "release" / "archietect")
QFILE = Path(__file__).resolve().parent / "queries.json"
OUTDIR = Path(__file__).resolve().parent

queries = json.loads(QFILE.read_text())
for spec in queries.values():
    spec["root"] = str(REPO_ROOT / spec["root"])

def run_archietect(root, term):
    t0 = time.time()
    p = subprocess.run([ARCH, "--root", root, "concept", term], capture_output=True, text=True, timeout=30)
    t1 = time.time()
    try:
        out = json.loads(p.stdout)
    except Exception:
        out = {"_raw": p.stdout, "_stderr": p.stderr}
    return {"elapsed_s": round(t1 - t0, 4), "result": out}

def run_ame(root, term, task_intent):
    ctx = ProjectContext(Path(root))
    # NOTE: calibration showed AME's lexical-fallback retrieval (semantic/vector
    # disabled by default) is highly sensitive to task-string phrasing -- long,
    # natural-language asks ("Does X already exist in this codebase? Where is
    # it defined and used?") reliably scored 0 results even when X is indexed,
    # while a short, symbol-forward task consistently surfaces results. Using
    # the short form here to give AME its best empirically-observed case,
    # applied uniformly to every query (existing, absent, and ambiguous alike)
    # for fairness. See calibration transcript in debug1*.json/.err.
    task = f"Implement {term}"
    inp = RetrieveContextInput(
        task=task,
        task_intent=task_intent,
        current_symbols=[term],
        token_budget=6000,
    )
    t0 = time.time()
    out = tool_retrieve_agent_context(ctx, inp)
    t1 = time.time()
    # compact summary: file paths / titles surfaced, counts
    def summarize(section):
        items = []
        for n in out.get(section, []):
            items.append({
                "title": n.get("title") or n.get("name"),
                "content_snip": (n.get("content") or n.get("summary") or "")[:160],
                "source_path": n.get("source_path") or n.get("file_path") or n.get("path"),
            })
        return items

    knowledge_items = []
    for k in out.get("knowledge_chunks", []) + out.get("multigranular_chunks", []):
        knowledge_items.append({
            "path": k.get("path") or k.get("file_path"),
            "start_line": k.get("start_line"),
            "end_line": k.get("end_line"),
            "snippet": (k.get("content") or "")[:160],
        })

    trace_paths = []
    for t in out.get("retrieval_trace", []):
        p = t.get("path")
        if p:
            trace_paths.append({"path": p, "start_line": t.get("start_line"), "end_line": t.get("end_line"), "score": t.get("score")})

    return {
        "elapsed_s": round(t1 - t0, 4),
        "task_intent": task_intent,
        "memory_results_count": out.get("memory_results_count"),
        "knowledge_results_count": out.get("knowledge_results_count"),
        "multigranular_results_count": out.get("multigranular_results_count"),
        "cache_hit": out.get("cache_hit"),
        "retrieval_mode": out.get("meta", {}).get("retrieval_mode"),
        "modules": summarize("modules"),
        "architecture": summarize("architecture"),
        "decisions": summarize("decisions"),
        "knowledge_items": knowledge_items[:8],
        "trace_paths": trace_paths[:10],
    }


def main():
    for repo, spec in queries.items():
        root = spec["root"]
        results = []
        for q in spec["queries"]:
            term = q["term"]
            entry = {"term": term, "ground_truth": q["gt"], "note": q["note"]}
            entry["archietect"] = run_archietect(root, term)
            entry["ame_architecture_review"] = run_ame(root, term, "architecture_review")
            entry["ame_feature_implementation"] = run_ame(root, term, "feature_implementation")
            results.append(entry)
            print(f"done: {repo} / {term}", file=sys.stderr)
        outfile = OUTDIR / f"result_{repo}.json"
        outfile.write_text(json.dumps(results, indent=2, default=str))
        print(f"wrote {outfile}", file=sys.stderr)


if __name__ == "__main__":
    main()
