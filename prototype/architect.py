#!/usr/bin/env python3
"""architect — an architectural memory engine. v0 prototype.

Answers, for any codebase: "does this concept already exist, what is canonical,
what is the evidence, and what is the smallest correct change?"

THE NON-NEGOTIABLE PRINCIPLE
----------------------------
The Architect never invents architectural facts. Every answer carries its
evidence, and every piece of evidence is labelled with its STRENGTH:

    DECLARED  — read from a schema declaration (schema.prisma, models.py,
                CREATE TABLE). The project itself asserts this. Strongest.
    USED      — observed in source as a real access (prisma.model.*,
                Model.objects.*, INSERT INTO ...). The code demonstrably
                touches it.
    NAMED     — name resemblance only. The weakest tier, and it says so.

When only NAMED evidence exists, the verdict is UNKNOWN with
"needs human confirmation" — never a confident answer built on a filename.
An engine that answers confidently from weak evidence is worse than grep,
because it *looks* like knowledge. (Lesson paid for: a substring match once
reported a broken pipeline as healthy because '%sd%' matched 'USD'.)

WHY NO AI INSIDE
----------------
Deterministic on purpose. The LLM is a client of this engine, never a
component of it: the model reasons ON TOP of architectural facts; it does not
get to replace them with intuition — intuition is what produces duplicate
architecture in the first place.

v0 extractors: Prisma, Django models, raw SQL (CREATE TABLE). That covers a
large share of real projects; everything else degrades HONESTLY to NAMED
evidence rather than pretending.
"""

import json
import os
import re
import sys
from collections import defaultdict

SKIP_DIRS = {"node_modules", ".git", ".next", "target", "dist", "build",
             "__pycache__", ".venv", "venv", ".turbo", "coverage", ".cache"}
SRC_EXTS = {".ts", ".tsx", ".js", ".jsx", ".py", ".rs", ".go", ".java", ".prisma", ".sql"}
MAX_FILES = 20000


# ── word matching ────────────────────────────────────────────────────────────
# Prefix-sharing with bounded suffixes, so story/stories match and
# story/history do not. Substring matching is actively wrong here.

def same_word(a: str, b: str) -> bool:
    a, b = a.lower(), b.lower()
    if a == b:
        return True
    shared = 0
    for x, y in zip(a, b):
        if x != y:
            break
        shared += 1
    shorter = min(len(a), len(b))
    return (shared >= 4 and shared * 10 >= shorter * 7
            and len(a) - shared <= 3 and len(b) - shared <= 3)


def name_tokens(name: str):
    """Split snake_case AND camelCase into tokens."""
    s = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", name)
    return [t for t in re.split(r"[^A-Za-z0-9]+|_", s) if t]


def names_concept(name: str, term: str) -> bool:
    return any(same_word(tok, term) for tok in name_tokens(name))


# ── model: one graph ─────────────────────────────────────────────────────────

class Project:
    def __init__(self, root):
        self.root = os.path.abspath(root)
        # concept name -> {"declared_in": [(file, kind)], "fields": [...],
        #                  "relations": [other concepts], "table": mapped name}
        self.concepts = {}
        # concept -> [(file, kind)] observed accesses
        self.usage = defaultdict(list)
        self.files_scanned = 0
        self.declaration_sources = []

    def rel(self, path):
        return os.path.relpath(path, self.root)


# ── extractors (DECLARED tier) ───────────────────────────────────────────────

def extract_prisma(proj: Project, path: str):
    text = open(path, encoding="utf-8", errors="replace").read()
    proj.declaration_sources.append((proj.rel(path), "prisma"))
    for m in re.finditer(r"^model\s+(\w+)\s*\{(.*?)^\}", text, re.M | re.S):
        name, body = m.group(1), m.group(2)
        fields, relations = [], []
        table = None
        tmap = re.search(r'@@map\("([^"]+)"\)', body)
        if tmap:
            table = tmap.group(1)
        for line in body.splitlines():
            line = line.strip()
            if not line or line.startswith("//") or line.startswith("@@"):
                continue
            fm = re.match(r"(\w+)\s+(\w+)(\[\])?(\?)?", line)
            if fm:
                fields.append(fm.group(1))
                ftype = fm.group(2)
                # a field whose type is another Model is a declared relation
                if re.match(r"^[A-Z]", ftype) and ftype not in (
                        "String", "Int", "BigInt", "Float", "Decimal",
                        "Boolean", "DateTime", "Json", "Bytes"):
                    relations.append(ftype)
        c = proj.concepts.setdefault(name, {
            "declared_in": [], "fields": [], "relations": [], "table": None})
        c["declared_in"].append((proj.rel(path), "prisma"))
        c["fields"] = fields
        c["relations"] = sorted(set(relations))
        c["table"] = table or name
    # enums are concepts too, weaker shape
    for m in re.finditer(r"^enum\s+(\w+)\s*\{", text, re.M):
        name = m.group(1)
        c = proj.concepts.setdefault(name, {
            "declared_in": [], "fields": [], "relations": [], "table": None})
        c["declared_in"].append((proj.rel(path), "prisma-enum"))


def extract_django(proj: Project, path: str):
    text = open(path, encoding="utf-8", errors="replace").read()
    if "models.Model" not in text and "models.py" not in path:
        return
    proj.declaration_sources.append((proj.rel(path), "django"))
    for m in re.finditer(
            r"^class\s+(\w+)\s*\(([^)]*models\.Model[^)]*|[^)]*Model[^)]*)\)\s*:",
            text, re.M):
        name = m.group(1)
        # body = until next top-level class/def
        start = m.end()
        nxt = re.search(r"^\S", text[start:], re.M)
        body = text[start:start + nxt.start()] if nxt else text[start:]
        fields = re.findall(r"^\s{4}(\w+)\s*=\s*models\.", body, re.M)
        relations = re.findall(
            r"models\.(?:ForeignKey|OneToOneField|ManyToManyField)\(\s*['\"]?(\w+)", body)
        c = proj.concepts.setdefault(name, {
            "declared_in": [], "fields": [], "relations": [], "table": None})
        c["declared_in"].append((proj.rel(path), "django"))
        c["fields"] = fields
        c["relations"] = sorted(set(r for r in relations if r != "self"))
        # Django default table: app_modelname; we don't guess the app label —
        # a wrong table name stated confidently is worse than none.


def extract_sql(proj: Project, path: str):
    text = open(path, encoding="utf-8", errors="replace").read()
    hits = re.findall(
        r"create\s+table\s+(?:if\s+not\s+exists\s+)?[\"'`]?(\w+)", text, re.I)
    if hits:
        proj.declaration_sources.append((proj.rel(path), "sql"))
    for t in hits:
        c = proj.concepts.setdefault(t, {
            "declared_in": [], "fields": [], "relations": [], "table": t})
        c["declared_in"].append((proj.rel(path), "sql"))


# ── usage scan (USED tier) ───────────────────────────────────────────────────

def scan_usage(proj: Project, path: str, text: str):
    rel = proj.rel(path)
    lower = text.lower()
    for name, c in proj.concepts.items():
        lname = name[0].lower() + name[1:] if name else name
        kinds = []
        # Prisma client access: prisma.user. / db.user. / tx.user.
        if re.search(r"\b(?:prisma|db|tx|client)\." + re.escape(lname) + r"\.", text):
            kinds.append("prisma-client")
        # Django ORM access: User.objects.
        if name + ".objects." in text:
            kinds.append("django-orm")
        # raw SQL against the mapped table
        table = c.get("table")
        if table and re.search(
                r"(?:insert\s+into|update|from|join)\s+[\"'`]?" + re.escape(table.lower()) + r"\b",
                lower):
            kinds.append("raw-sql")
        for k in kinds:
            proj.usage[name].append((rel, k))


# ── scan driver ──────────────────────────────────────────────────────────────

def scan(root: str) -> Project:
    proj = Project(root)
    src_files = []
    for dirpath, dirnames, filenames in os.walk(proj.root):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for f in filenames:
            p = os.path.join(dirpath, f)
            ext = os.path.splitext(f)[1]
            if ext not in SRC_EXTS:
                continue
            src_files.append(p)
            if len(src_files) >= MAX_FILES:
                break
    # pass 1: declarations
    for p in src_files:
        f = os.path.basename(p)
        try:
            if f.endswith(".prisma"):
                extract_prisma(proj, p)
            elif f == "models.py":
                extract_django(proj, p)
            elif f.endswith(".sql"):
                extract_sql(proj, p)
        except Exception as e:
            print(f"  ! extractor failed on {proj.rel(p)}: {e}", file=sys.stderr)
    # pass 2: usage (only meaningful once concepts exist)
    for p in src_files:
        if os.path.basename(p).endswith((".prisma",)):
            continue
        try:
            text = open(p, encoding="utf-8", errors="replace").read()
        except Exception:
            continue
        proj.files_scanned += 1
        scan_usage(proj, p, text)
    return proj


# ── queries ──────────────────────────────────────────────────────────────────

def q_concept(proj: Project, term: str) -> dict:
    term = term.strip()
    # DECLARED matches
    declared = [n for n in proj.concepts if names_concept(n, term)]
    # NAMED-only fallback: file basenames resembling the term
    named_files = []
    if not declared:
        for dirpath, dirnames, filenames in os.walk(proj.root):
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
            for f in filenames:
                stem = os.path.splitext(f)[0]
                if os.path.splitext(f)[1] in SRC_EXTS and names_concept(stem, term):
                    named_files.append(proj.rel(os.path.join(dirpath, f)))
        named_files = named_files[:10]

    if declared:
        # canonical = most USED declared concept; ties -> most relations
        ranked = sorted(
            declared,
            key=lambda n: (len(proj.usage.get(n, [])), len(proj.concepts[n]["relations"])),
            reverse=True)
        canon = ranked[0]
        c = proj.concepts[canon]
        used = proj.usage.get(canon, [])
        evidence = (
            [{"tier": "DECLARED", "what": f"{kind} declaration in {f}"}
             for f, kind in c["declared_in"]] +
            [{"tier": "USED", "what": f"{kind} access in {f}"}
             for f, kind in used[:8]])
        verdict = "ACTIVE" if used else "DECLARED_ONLY"
        return {
            "concept": term, "verdict": verdict, "canonical": canon,
            "table": c.get("table"),
            "fields": c["fields"][:15], "relations": c["relations"],
            "competing": [n for n in ranked[1:6]],
            "evidence": evidence,
            "confidence": "high" if used else
                          "medium — declared but no observed access; may be scaffolding",
            "recommendation":
                f"'{term}' already exists as model '{canon}'. Extend it; "
                "do not create a second implementation."
                if used else
                f"'{term}' is declared as '{canon}' but nothing observably uses it. "
                "Confirm whether it is scaffolding before extending OR replacing.",
        }
    if named_files:
        return {
            "concept": term, "verdict": "UNKNOWN", "canonical": None,
            "evidence": [{"tier": "NAMED",
                          "what": f"filename resemblance only: {f}"}
                         for f in named_files],
            "confidence": "low — name resemblance is not an architectural fact",
            "recommendation":
                "Needs human confirmation. Files with similar names exist but no "
                "schema declares this concept — inspect them before building anything.",
        }
    return {
        "concept": term, "verdict": "ABSENT", "canonical": None, "evidence": [],
        "confidence": "high — no declaration, no usage, no name resemblance",
        "recommendation": "Genuinely new for this project. Building it is justified.",
    }


STOP = {"want", "need", "make", "build", "better", "improve", "with", "that",
        "this", "have", "from", "into", "would", "should", "could", "will",
        "more", "less", "system", "support", "please", "about", "when", "then",
        "them", "track", "tracking", "show", "view", "page", "data", "info",
        "some", "every", "feature", "the", "and", "for", "add", "create"}


def q_intent(proj: Project, intent: str) -> dict:
    terms, seen = [], set()
    for w in re.split(r"[^A-Za-z0-9]+", intent.lower()):
        if len(w) >= 4 and w not in STOP and w not in seen:
            seen.add(w)
            terms.append(w)
    terms = terms[:8]
    extend, create, unknown = [], [], []
    for t in terms:
        r = q_concept(proj, t)
        if r["verdict"] in ("ACTIVE", "DECLARED_ONLY"):
            extend.append({"concept": t, "canonical": r["canonical"],
                           "verdict": r["verdict"],
                           "relations": r.get("relations", [])})
        elif r["verdict"] == "UNKNOWN":
            unknown.append({"concept": t,
                            "note": "name-resemblance only — confirm by hand"})
        else:
            create.append(t)
    if extend:
        summary = (f"{len(extend)} concept(s) already exist: extend "
                   + ", ".join(e["canonical"] for e in extend)
                   + ". " + ("Nothing genuinely new required."
                             if not create else
                             f"Only genuinely new: {', '.join(create)}."))
    elif create and not unknown:
        summary = "No named concept exists here — this intent is greenfield for this project."
    else:
        summary = ("Nothing matched declarations. Either the vocabulary differs "
                   "from the project's or this is new territory.")
    return {"intent": intent, "recognised": terms, "extend": extend,
            "create": create, "needs_confirmation": unknown,
            "smallest_correct_change": summary}


def q_impact(proj: Project, term: str) -> dict:
    r = q_concept(proj, term)
    if not r.get("canonical"):
        return {"target": term, "impact": "unknown — concept not declared here",
                "detail": r}
    canon = r["canonical"]
    users = proj.usage.get(canon, [])
    files = sorted({f for f, _ in users})
    dependents = [n for n, c in proj.concepts.items()
                  if canon in c["relations"]]
    return {
        "target": canon,
        "severity": ("HIGH — widely used and other models declare relations to it"
                     if len(files) > 8 or len(dependents) > 3 else
                     "MODERATE — several touchpoints" if files or dependents else
                     "NONE OBSERVED — declared but nothing seen touching it"),
        "used_by_files": files[:20],
        "declared_dependents": dependents,
        "evidence_note": "used_by = observed ORM/SQL access (USED tier); "
                         "dependents = schema-declared relations (DECLARED tier).",
    }


def q_scan_summary(proj: Project) -> dict:
    used = {n: len(v) for n, v in proj.usage.items() if v}
    dead = [n for n in proj.concepts
            if n not in used and
            any(k != "prisma-enum" for _, k in proj.concepts[n]["declared_in"])]
    return {
        "root": proj.root,
        "files_scanned": proj.files_scanned,
        "declaration_files": sorted({f for f, _ in proj.declaration_sources}),
        "concepts_declared": len(proj.concepts),
        "concepts_with_observed_usage": len(used),
        "declared_but_never_observed_in_use": sorted(dead)[:25],
        "note": "'never observed in use' is evidence of absence at USED tier only "
                "— access styles v0 doesn't parse (raw drivers, GraphQL "
                "resolvers, services in other repos) would be invisible. "
                "Stated so it cannot be mistaken for proof of death.",
    }


def main():
    if len(sys.argv) < 3:
        print("usage: architect.py <scan|concept|intent|impact> --root DIR [query]")
        sys.exit(1)
    cmd = sys.argv[1]
    args = sys.argv[2:]
    root, query = None, []
    i = 0
    while i < len(args):
        if args[i] == "--root":
            root = args[i + 1]
            i += 2
        else:
            query.append(args[i])
            i += 1
    if not root:
        print("--root required")
        sys.exit(1)
    proj = scan(root)
    q = " ".join(query)
    out = {"scan": lambda: q_scan_summary(proj),
           "concept": lambda: q_concept(proj, q),
           "intent": lambda: q_intent(proj, q),
           "impact": lambda: q_impact(proj, q)}.get(cmd)
    if not out:
        print(f"unknown command {cmd}")
        sys.exit(1)
    print(json.dumps(out(), indent=1))


if __name__ == "__main__":
    main()
