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
            <a href="#problem">Why</a>
            <a href="#how">How it works</a>
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
            The <span className="accent">memory</span>
            <br />
            your codebase
            <br />
            never had
          </h1>
          <p className="sub">
            Archietect is an evidence-backed record of what a codebase <b>is</b> — what
            exists, what&rsquo;s canonical, who uses it, why it&rsquo;s shaped this way. One
            engine, queried identically by CLI, REST, and MCP, so an AI agent, a human, and
            a CI job stop independently reconstructing the same facts every single session.
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

        {/* ============================== SHEET 01 — PROBLEM ============================== */}
        <section className="sheet" id="problem">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 01</span>
              <h2>The rediscovery tax</h2>
            </div>
            <p className="sheet-kicker">
              An independent account from Kiro, an AI coding agent, after using Archietect on
              itself for the first time — commands run live, outputs real. The full write-up
              is{" "}
              <a href="https://github.com/Neville777/Archietect/blob/main/AI_AGENT_REPORT.md">
                in the repo
              </a>
              .
            </p>
            <div className="quotes">
              <blockquote>
                &ldquo;Every time an AI agent starts a conversation, it knows nothing about the
                project&hellip; the agent reads README.md, scans files one by one, builds a
                mental model, and presents that model as if it were fact. The user cannot
                verify it. It does not persist. Nothing accumulates.&rdquo;
                <cite>Kiro — before Archietect</cite>
              </blockquote>
              <blockquote>
                &ldquo;That is the architectural state of the project — not my reconstruction
                of it. It is reproducible. You can run the same command and get the same
                answer. It does not evaporate when this conversation ends.&rdquo;
                <cite>
                  Kiro — after running <code>archietect doctor</code>
                </cite>
              </blockquote>
            </div>
          </div>
        </section>

        {/* ============================== SHEET 02 — HOW IT WORKS ============================== */}
        <section className="sheet" id="how">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 02</span>
              <h2>How it answers</h2>
            </div>
            <p className="sheet-kicker">
              Every answer is ranked by evidence tier, never invented, and ends in one of four
              verdicts — the same shape whether you&rsquo;re asking from a terminal or an AI is
              asking on your behalf.
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

        {/* ============================== SHEET 03 — COVERAGE ============================== */}
        <section className="sheet" id="coverage">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 03</span>
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

        {/* ============================== SHEET 04 — BENCHMARK ============================== */}
        <section className="sheet" id="benchmark">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 04</span>
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

        {/* ============================== SHEET 05 — GET IT ============================== */}
        <section className="sheet" id="get-it">
          <div className="wrap">
            <div className="sheet-head">
              <span className="sheet-no">SHEET 05</span>
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
