"use client";

import { useState } from "react";

const INSTALL_CMD =
  "curl -fsSL https://raw.githubusercontent.com/Neville777/Archietect/main/packaging/install.sh | sh";

export default function Page() {
  const [copied, setCopied] = useState(false);

  function copyInstall() {
    navigator.clipboard
      .writeText(INSTALL_CMD)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1600);
      })
      .catch(() => {});
  }

  return (
    <>
      <header className="site">
        <div className="wrap nav">
          <a className="wordmark" href="#top">
            ARCHIE<span className="dot">·</span>TECT
          </a>
          <nav className="navlinks">
            <a href="#problem">The problem</a>
            <a href="#evidence">Evidence</a>
            <a href="#coverage">Coverage</a>
            <a href="#get-it">Install</a>
            <a className="cta" href="https://github.com/Neville777/Archietect">
              GitHub
            </a>
          </nav>
        </div>
      </header>

      <main id="top">
        <section className="hero wrap">
          <div className="eyebrow">Deterministic · offline · no AI inside</div>
          <h1 className="headline">
            The architectural
            <br />
            <span className="accent">memory</span> your
            <br />
            system never had
          </h1>
          <p className="sub">
            Most systems know a lot about themselves — git knows what changed, Docker knows
            what&rsquo;s running, the source tree knows what files exist. None of that is a
            <b> memory</b> of the system. Archietect is: a persistent, evidence-backed record
            of what exists, how it relates, and what remains unknown — queried identically by
            CLI, REST, and MCP, so a human, an AI agent, and a CI job stop independently
            rediscovering the same facts every single session.
          </p>

          <div className="install">
            <div className="install-box">
              <span className="prompt">$</span>
              <code>{INSTALL_CMD}</code>
              <button className="copy-btn" onClick={copyInstall}>
                {copied ? "Copied" : "Copy"}
              </button>
            </div>
          </div>
          <div className="install-alt">
            Rust already installed? <code>cargo install archietect</code> works too.
          </div>

          <div className="diagram-wrap reveal">
            <svg
              viewBox="0 0 960 340"
              xmlns="http://www.w3.org/2000/svg"
              role="img"
              aria-label="Diagram: humans and AI tools both query Archietect over CLI, REST, or MCP; Archietect answers from architectural state and a system-wide registry, both derived from source code, schemas, and ADRs."
            >
              <defs>
                <marker
                  id="arrow"
                  viewBox="0 0 8 8"
                  refX="7"
                  refY="4"
                  markerWidth="7"
                  markerHeight="7"
                  orient="auto-start-reverse"
                >
                  <path d="M0,0 L8,4 L0,8 z" fill="var(--line-strong)"></path>
                </marker>
              </defs>
              <g fontFamily="IBM Plex Mono, monospace">
                <rect x="60" y="24" width="150" height="46" fill="none" stroke="var(--line-strong)"></rect>
                <text x="135" y="52" textAnchor="middle" fontSize="13" fill="var(--ink-soft)" letterSpacing="0.5">
                  HUMAN
                </text>

                <rect x="260" y="24" width="230" height="46" fill="none" stroke="var(--line-strong)"></rect>
                <text x="375" y="46" textAnchor="middle" fontSize="12.5" fill="var(--ink-soft)" letterSpacing="0.5">
                  CLAUDE · GPT · CURSOR
                </text>
                <text x="375" y="62" textAnchor="middle" fontSize="10.5" fill="var(--accent)" letterSpacing="0.5">
                  INTELLIGENCE LIVES HERE
                </text>

                <line x1="135" y1="70" x2="135" y2="112" stroke="var(--line-strong)" markerEnd="url(#arrow)"></line>
                <line x1="375" y1="70" x2="375" y2="112" stroke="var(--line-strong)" markerEnd="url(#arrow)"></line>

                <rect
                  x="60"
                  y="114"
                  width="430"
                  height="56"
                  fill="var(--accent-soft)"
                  stroke="var(--accent)"
                  strokeWidth="1.5"
                ></rect>
                <text x="275" y="138" textAnchor="middle" fontSize="14" fontWeight="600" fill="var(--ink)" letterSpacing="0.5">
                  ARCHIETECT
                </text>
                <text x="275" y="156" textAnchor="middle" fontSize="11" fill="var(--muted)" letterSpacing="1">
                  CLI &#183; REST &#183; MCP &#8212; ONE ENGINE
                </text>

                <line x1="160" y1="170" x2="160" y2="208" stroke="var(--line-strong)" markerEnd="url(#arrow)"></line>
                <line x1="390" y1="170" x2="390" y2="208" stroke="var(--line-strong)" markerEnd="url(#arrow)"></line>

                <rect x="60" y="210" width="200" height="70" fill="var(--surface)" stroke="var(--line-strong)"></rect>
                <text x="160" y="232" textAnchor="middle" fontSize="12" fontWeight="600" fill="var(--ink)">
                  ARCHITECTURAL STATE
                </text>
                <text x="160" y="248" textAnchor="middle" fontSize="10" fill="var(--muted)">
                  laws · concepts · decisions
                </text>
                <text x="160" y="262" textAnchor="middle" fontSize="10" fill="var(--muted)">
                  evidence · history
                </text>

                <rect x="290" y="210" width="200" height="70" fill="var(--surface)" stroke="var(--line-strong)"></rect>
                <text x="390" y="232" textAnchor="middle" fontSize="12" fontWeight="600" fill="var(--ink)">
                  SYSTEM REGISTRY
                </text>
                <text x="390" y="248" textAnchor="middle" fontSize="10" fill="var(--muted)">
                  every known project
                </text>
                <text x="390" y="262" textAnchor="middle" fontSize="10" fill="var(--muted)">
                  who&rsquo;s active, and where
                </text>

                <line x1="275" y1="280" x2="275" y2="310" stroke="var(--line-strong)" markerEnd="url(#arrow)"></line>
                <text x="275" y="322" textAnchor="middle" fontSize="11" fill="var(--muted)" letterSpacing="0.5">
                  SOURCE CODE · SCHEMAS · ADRS — TRUTH LIVES HERE
                </text>

                <g stroke="var(--line)" strokeDasharray="2 3">
                  <line x1="560" y1="24" x2="560" y2="326"></line>
                </g>
                <g fontSize="11" fill="var(--muted)">
                  <text x="580" y="45">19 languages structurally</text>
                  <text x="580" y="63">parsed, schema-aware for</text>
                  <text x="580" y="81">13 ORMs/frameworks.</text>

                  <text x="580" y="118">15 active laws, each tied</text>
                  <text x="580" y="136">to a real bug this engine</text>
                  <text x="580" y="154">once produced.</text>

                  <text x="580" y="191">Every verdict: DECLARED &gt;</text>
                  <text x="580" y="209">USED &gt; NAMED. Never</text>
                  <text x="580" y="227">invented.</text>

                  <text x="580" y="264">Read-only against your</text>
                  <text x="580" y="282">code. Nothing here ever</text>
                  <text x="580" y="300">edits your working tree.</text>
                </g>
              </g>
            </svg>
            <div className="titleblock">
              <span>
                <strong>FIG. 1</strong> — DATA FLOW
              </span>
              <span>SCALE: NONE · TIER: DECLARED</span>
              <span>ARCHIETECT / README.MD</span>
            </div>
          </div>
        </section>

        {/* ============================== SHEET 01 — THE PROBLEM ============================== */}
        <section className="sheet" id="problem">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 01</span>
              <h2>The missing memory layer</h2>
            </div>
            <p className="sheet-kicker">
              Most systems know a lot about themselves. Git knows what changed. Docker knows
              what&rsquo;s running. The source tree knows what files exist. None of that is
              really a <b>memory</b> of the system.
            </p>
            <div className="prose">
              <p>
                Ask a new tool whether a particular service exists, what depends on it, or
                whether something is actually being used, and it usually starts searching from
                scratch. That works — it also means the same system gets rediscovered over and
                over again. An AI agent searches the repository. A developer searches it again
                later. A CI job builds its own representation. Each one produces a temporary
                understanding and then throws most of it away.
              </p>
              <p>
                <strong>
                  The problem isn&rsquo;t that the information doesn&rsquo;t exist. The problem
                  is that the system doesn&rsquo;t have a place where established knowledge
                  about itself persists.
                </strong>{" "}
                That&rsquo;s the problem architectural memory is designed to address.
              </p>
            </div>

            <div className="quotes" style={{ marginTop: "32px" }}>
              <blockquote>
                &ldquo;Every time an AI agent starts a conversation, it knows nothing about the
                project&hellip; the agent reads README.md, scans files one by one, builds a
                mental model, and presents that model as if it were fact. The user cannot
                verify it. It does not persist. Nothing accumulates.&rdquo;
                <cite>Kiro, an AI coding agent — before Archietect</cite>
              </blockquote>
              <blockquote>
                &ldquo;That is the architectural state of the project — not my reconstruction
                of it. It is reproducible. You can run the same command and get the same
                answer. It does not evaporate when this conversation ends.&rdquo;
                <cite>
                  Kiro — after running <code>archietect doctor</code>,{" "}
                  <a href="https://github.com/Neville777/Archietect/blob/main/AI_AGENT_REPORT.md">
                    full account in the repo
                  </a>
                </cite>
              </blockquote>
            </div>
          </div>
        </section>

        {/* ============================== SHEET 02 — SEARCH VS MEMORY ============================== */}
        <section className="sheet" id="how">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 02</span>
              <h2>Search finds things. Memory keeps them.</h2>
            </div>
            <p className="sheet-kicker">
              A conventional tool searches filenames and source code, finds something that
              looks relevant, and leaves the consumer to decide what it means. Architectural
              memory represents a concept as a resource and keeps the evidence with it — so the
              answer to &ldquo;does this exist&rdquo; is never just a yes or no.
            </p>

            <div className="split">
              <div>
                <div className="tierlist">
                  <div className="tier">
                    <span className="name">DECLARED</span>
                    <span className="desc">
                      the project&rsquo;s own schema asserts it — a table, a model, a route.
                    </span>
                  </div>
                  <div className="tier">
                    <span className="name">USED</span>
                    <span className="desc">
                      code observably touches it, even with no formal declaration.
                    </span>
                  </div>
                  <div className="tier">
                    <span className="name">NAMED</span>
                    <span className="desc">
                      name resemblance only — flagged UNKNOWN, needs a human.
                    </span>
                  </div>
                </div>
                <div className="verdicts">
                  <div className="verdict-card" style={{ ["--v" as any]: "var(--ok)" }}>
                    <div className="label">Active</div>
                    <p>Extend it. Do not rebuild.</p>
                  </div>
                  <div className="verdict-card" style={{ ["--v" as any]: "var(--warn)" }}>
                    <div className="label">Declared only</div>
                    <p>Confirm before extending.</p>
                  </div>
                  <div className="verdict-card" style={{ ["--v" as any]: "var(--warn)" }}>
                    <div className="label">Unknown</div>
                    <p>Name match only — ask a human.</p>
                  </div>
                  <div className="verdict-card" style={{ ["--v" as any]: "var(--bad)" }}>
                    <div className="label">Absent</div>
                    <p>Building it is justified.</p>
                  </div>
                </div>
              </div>

              <div className="term reveal">
                <div className="term-bar">
                  <span></span>
                  <span></span>
                  <span></span>
                </div>
                <div className="term-body">
                  <pre>
                    <span className="p">$</span> <span className="cmd">archietect concept PaymentRefundService</span>
                    {"\n"}
                    {"{\n"}
                    {"  "}
                    <span className="k">&quot;verdict&quot;</span>: <span className="s">&quot;ABSENT&quot;</span>,{"\n"}
                    {"  "}
                    <span className="k">&quot;confidence&quot;</span>:{" "}
                    <span className="s">
                      &quot;high — no declaration, no observed usage, no name resemblance&quot;
                    </span>
                    ,{"\n"}
                    {"  "}
                    <span className="k">&quot;recommendation&quot;</span>:{" "}
                    <span className="s">&quot;Genuinely new for this project. Building it is justified.&quot;</span>
                    {"\n}"}
                  </pre>
                  <pre>
                    <span className="p">$</span> <span className="cmd">archietect concept Index</span>
                    {"\n"}
                    {"{\n"}
                    {"  "}
                    <span className="k">&quot;verdict&quot;</span>: <span className="s">&quot;DECLARED_ONLY&quot;</span>,{"\n"}
                    {"  "}
                    <span className="k">&quot;confidence&quot;</span>:{" "}
                    <span className="s">&quot;medium — declared but no observed access; may be scaffolding&quot;</span>
                    ,{"\n"}
                    {"  "}
                    <span className="k">&quot;recommendation&quot;</span>:{" "}
                    <span className="s">
                      &quot;Confirm whether it is scaffolding before extending OR replacing.&quot;
                    </span>
                    {"\n}"}
                  </pre>
                </div>
              </div>
            </div>
          </div>
        </section>

        {/* ============================== SHEET 03 — EVIDENCE & RELATIONSHIPS ============================== */}
        <section className="sheet" id="evidence">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 03</span>
              <h2>Evidence has to survive the query</h2>
            </div>
            <p className="sheet-kicker">
              A memory that only stores conclusions isn&rsquo;t particularly trustworthy — it
              needs to remember <em>why</em> the conclusion exists. A fact can be declared,
              observed, derived, or inferred. Those aren&rsquo;t interchangeable, and a
              relationship has the same problem as a concept does.
            </p>
            <div className="prose">
              <p>
                Suppose a system contains a dependency: an order service that depends on Redis.
                That relationship needs its own evidence, separate from the two things it
                connects — it might come from a Docker Compose definition, from configuration,
                or from an observed connection. The existence of the order service and the
                existence of Redis do not, by themselves, establish that dependency.
              </p>
              <p>
                It&rsquo;s a small distinction, but it changes the shape of the memory. It
                isn&rsquo;t just a collection of things — it&rsquo;s a representation of{" "}
                <strong>things, relationships, and the evidence supporting both.</strong>
              </p>
            </div>
          </div>
        </section>

        {/* ============================== SHEET 04 — BOUNDARIES ============================== */}
        <section className="sheet" id="boundaries">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 04</span>
              <h2>A memory needs a boundary</h2>
            </div>
            <p className="sheet-kicker">
              A memory system can become dangerous if it treats absence of evidence as evidence
              of absence — and this matters most once it can observe more than source code. A
              machine may contain documents, photos, and message stores alongside its
              repositories. Knowing something exists doesn&rsquo;t mean it should be opened.
            </p>
            <div className="prose">
              <p>
                The documents domain can establish metadata about files without reading their
                contents. The photos domain can establish metadata without looking at the
                pixels. The messages domain can detect known local message stores (iMessage,
                Signal, WhatsApp, Slack, Discord) without opening the underlying database. That
                means Archietect can know <em>a message store exists</em> without ever claiming
                to know what was said. That isn&rsquo;t a missing feature — it&rsquo;s an
                intentional boundary, and it holds in both directions: if a photo has never been
                inspected, Archietect cannot infer what&rsquo;s depicted in it. The memory
                records the boundary instead of silently crossing it.
              </p>
            </div>
            <div className="verdicts" style={{ marginTop: "28px" }}>
              <div className="verdict-card" style={{ ["--v" as any]: "var(--accent)" }}>
                <div className="label">Documents</div>
                <p>Filename, extension, size, mtime. Content never read.</p>
              </div>
              <div className="verdict-card" style={{ ["--v" as any]: "var(--accent)" }}>
                <div className="label">Photos</div>
                <p>Same metadata contract. Pixels never inspected.</p>
              </div>
              <div className="verdict-card" style={{ ["--v" as any]: "var(--accent)" }}>
                <div className="label">Messages</div>
                <p>Store existence and mtime only. Nothing opened or queried.</p>
              </div>
              <div className="verdict-card" style={{ ["--v" as any]: "var(--accent)" }}>
                <div className="label">Docker</div>
                <p>Live running/stopped state — the one domain that shells out at all.</p>
              </div>
            </div>

            <div className="tier" style={{ marginTop: "28px", gridTemplateColumns: "1fr" }}>
              <div>
                <span className="name">THE REGISTER</span>
                <p className="desc" style={{ marginTop: "6px" }}>
                  Once this exists, one query answers four questions together — what&rsquo;s
                  known, what&rsquo;s not known, why it&rsquo;s not known, and what&rsquo;s
                  allowed. If a domain is disabled, that explains a gap. If a concept is
                  declared but never observed in use, that&rsquo;s a different kind of gap.
                  <code style={{ marginLeft: "8px" }}>archietect register</code> — call it
                  before trusting any <code>ABSENT</code>.
                </p>
              </div>
            </div>
          </div>
        </section>

        {/* ============================== SHEET 05 — SCOPE & CONSUMERS ============================== */}
        <section className="sheet" id="scope">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 05</span>
              <h2>One memory, many consumers</h2>
            </div>
            <p className="sheet-kicker">
              Before a system can tell you what it is, it needs some basis for deciding{" "}
              <em>which</em> system you&rsquo;re talking about. Pointing a scanner at a parent
              directory containing several independent repositories and treating the whole
              thing as one project produces a false representation — so Archietect detects when
              a location looks like a workspace of many projects and warns before treating it as
              a single architectural scope. A genuine project root stays quiet; the scan itself
              is never blocked.
            </p>
            <div className="prose">
              <p>
                Archietect isn&rsquo;t built as memory for one particular AI. Claude, Cursor,
                Codex, a developer, a CI system, or a monitoring process can all query the same
                persistent representation instead of each maintaining its own. If the system has
                already established that three components depend on a particular service, every
                consumer gets that same relationship rather than independently reconstructing
                it.
              </p>
              <p>
                <strong>AI is a client of the memory. The memory doesn&rsquo;t need AI to
                exist.</strong>
              </p>
            </div>

            <div className="diagram-wrap reveal" style={{ marginTop: "32px" }}>
              <img src="/gui-demo.gif" alt="Archietect's GUI: overview with real hierarchy, a nested domain → file → concept drill-down, and the query tab answering a raw endpoint call." />
              <div className="titleblock">
                <span>
                  <strong>FIG. 2</strong> — THE GUI, LIVE
                </span>
                <span>SAME ENGINE, THIRD TRANSPORT</span>
                <span>ARCHIETECT / README.MD</span>
              </div>
            </div>
          </div>
        </section>

        {/* ============================== SHEET 06 — COVERAGE ============================== */}
        <section className="sheet" id="coverage">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 06</span>
              <h2>Structural coverage</h2>
            </div>
            <p className="sheet-kicker">
              Regex-based, not a real parser — a stated tradeoff. A language with no extractor
              returns <code>INSUFFICIENT_COVERAGE</code> and names the files worth reading,
              never a guessed answer.
            </p>
            <div className="cov-grid">
              {[
                ["Rust", "structs · enums · traits"],
                ["Python", "FastAPI · Flask · Django"],
                ["TypeScript / JS", "Express · NestJS · Next.js"],
                ["Vue", "SFC · Nuxt pages"],
                ["Go", "structs · interfaces"],
                ["Java / Kotlin", "Spring MVC"],
                ["Ruby", "Rails"],
                ["Elixir", "Phoenix"],
                ["PHP", "classes · interfaces"],
                ["C#", "ASP.NET Core"],
                ["Swift", "Vapor"],
                ["Objective-C", "@interface · @protocol"],
                ["C / C++", "structs · functions"],
                ["Scala", "classes · traits"],
                ["Dart", "classes · functions"],
                ["Haskell", "Yesod routes"],
                ["Clojure", "Compojure"],
                ["GraphQL", "types · operations"],
                ["Protocol Buffers", "gRPC services"],
              ].map(([lang, fw]) => (
                <div className="cov-cell" key={lang}>
                  <div className="lang">{lang}</div>
                  <div className="fw">{fw}</div>
                </div>
              ))}
            </div>
            <p className="cov-note">
              Schema layer additionally recognizes Prisma, Drizzle, TypeORM, Sequelize/Mongoose,
              Django, SQLAlchemy, pydantic/SQLModel, Rails/ActiveRecord, Eloquent, JPA, GORM,
              Ecto, and raw <code>CREATE TABLE</code> from any source.
            </p>
          </div>
        </section>

        {/* ============================== SHEET 07 — BENCHMARK ============================== */}
        <section className="sheet" id="benchmark">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 07</span>
              <h2>The benchmark</h2>
            </div>
            <p className="sheet-kicker">
              Against Agent Memory Engine, on surfacing a concept&rsquo;s real declaring file,
              across 3 public repos already sitting in <code>validation/</code>. Reproducible
              from a script in the repo — limitations stated there too.
            </p>
            <div className="bench">
              <div className="bench-card win">
                <div className="fig">15 / 15</div>
                <div className="name">Archietect</div>
                <div className="fine">correct declaring file</div>
              </div>
              <div className="bench-card">
                <div className="fig">5 / 15</div>
                <div className="name">Agent Memory Engine</div>
                <div className="fine">correct declaring file</div>
              </div>
            </div>
            <div className="links-row">
              <a
                className="btn"
                href="https://github.com/Neville777/Archietect/tree/main/benchmarks/vs-agent-memory-engine"
              >
                See the script &amp; raw results
              </a>
            </div>
          </div>
        </section>

        {/* ============================== SHEET 08 — GET IT ============================== */}
        <section className="sheet" id="get-it">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 08</span>
              <h2>Get it running</h2>
            </div>
            <p className="sheet-kicker">Three ways in, depending on how much terminal you want.</p>

            <div className="steps">
              <div className="step">
                <div>
                  <h3>Install, then point it at a project</h3>
                  <p>
                    First run indexes on the spot — no separate <code className="inline">init</code>{" "}
                    step. Every run after that is incremental.
                  </p>
                  <pre className="block">
                    {"curl -fsSL https://raw.githubusercontent.com/Neville777/Archietect/main/packaging/install.sh | sh\ncd /path/to/your-project\narchietect"}
                  </pre>
                </div>
              </div>
              <div className="step">
                <div>
                  <h3>Already have Rust?</h3>
                  <p>
                    Skips the install script&rsquo;s one extra step — auto-registering with
                    Claude Code / Gemini CLI.
                  </p>
                  <pre className="block">cargo install archietect</pre>
                </div>
              </div>
              <div className="step">
                <div>
                  <h3>Don&rsquo;t want a terminal at all</h3>
                  <p>
                    Native desktop app — <code className="inline">.deb</code> /{" "}
                    <code className="inline">.rpm</code> / AppImage for Linux,{" "}
                    <code className="inline">.dmg</code> for macOS (Intel + Apple Silicon),{" "}
                    <code className="inline">.msi</code> for Windows. Download, double-click,
                    done.
                  </p>
                </div>
              </div>
            </div>

            <div className="links-row">
              <a className="btn primary" href="https://github.com/Neville777/Archietect/releases/latest">
                Download the desktop app
              </a>
              <a className="btn" href="https://github.com/Neville777/Archietect#readme">
                Read the full docs
              </a>
              <a className="btn" href="https://crates.io/crates/archietect">
                View on crates.io
              </a>
            </div>
          </div>
        </section>
      </main>

      <footer className="wrap">
        <div className="fl">
          Business Source License 1.1 — free to run, modify, and use in production. Converts
          to Apache 2.0 on 2030-09-01.{" "}
          <a href="https://github.com/Neville777/Archietect/blob/main/LICENSE">Read the license</a>.
        </div>
        <div className="cols">
          <div className="col">
            <a href="https://github.com/Neville777/Archietect">GitHub</a>
            <a href="https://github.com/Neville777/Archietect/issues">Issues</a>
            <a href="https://github.com/Neville777/Archietect/blob/main/CONTRIBUTING.md">Contributing</a>
          </div>
          <div className="col">
            <a href="https://crates.io/crates/archietect">crates.io</a>
            <a href="https://github.com/Neville777/Archietect/releases">Releases</a>
            <a href="https://github.com/Neville777/Archietect/blob/main/SECURITY.md">Security</a>
          </div>
        </div>
      </footer>
    </>
  );
}
