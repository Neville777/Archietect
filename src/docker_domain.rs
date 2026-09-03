//! Second structured domain, added to test whether `resource::Resource`
//! generalizes past git — see SYSTEM_MEMORY.md's rollout, phase 3 (git) was
//! the first non-code domain; this is the second. Mirrors `git_domain.rs`'s
//! shape exactly: a raw `scan()` primitive, a gated `scan_if_allowed()` entry
//! point, module doc justifying each tier choice instead of asserting it.
//!
//! `scan`/`scan_if_allowed` below: static, declarative parsing only — no
//! `docker` CLI invocation, no daemon socket connection, same restraint
//! `git_domain.rs` shows by reading plaintext files instead of shelling out
//! to `git`. Every `Resource` these two produce is DECLARED, never OBSERVED:
//! they cannot see whether a container is actually running, only what a
//! Dockerfile or compose file asserts should exist.
//!
//! `scan_observed` below is the deliberate exception: it DOES shell out, to
//! `docker compose ... ps --format json --all`, because "is this declared
//! service actually running right now" is a live fact no static file can
//! ever carry — the same reason `git_domain.rs` reads `.git/HEAD` for the
//! one Observed fact it produces. It is a SEPARATE, explicit entry point,
//! never invoked from `scan`/`scan_if_allowed` — a live process call has
//! real cost and a real failure mode (daemon down, `docker` missing, a
//! hung/unresponsive engine) that a routine `archietect status` must never
//! pay or block on. A caller who wants live state asks for it by name.
//!
//! No YAML-parsing crate exists in this project's Cargo.toml today (checked
//! before writing this), and docker-compose's YAML is far less regular than
//! git's `[section "sub"]` config format `git_domain.rs` hand-parses — full
//! YAML has anchors, multi-line scalars, flow-style collections, none of
//! which a hand parser should attempt. Rather than add a new dependency for
//! a small, optional, structured-domain extractor, `compose_services` below
//! hand-parses only the common, simple block-style subset (2-space-indented
//! `services:` mapping, each service a further-indented mapping of scalar
//! `image:`/list `ports:` keys) and silently skips anything it doesn't
//! recognize — fails open, the same direction `query::guard` and
//! `scan_with_prior`'s `archietect.toml` parsing already fail open elsewhere
//! in this codebase. A compose file using anchors, multi-document YAML, or
//! other advanced features yields fewer (or zero) resources rather than a
//! wrong parse or a crash.

use crate::model::{Evidence, Tier};
use crate::resource::{Identity, Location, Resource};
use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The gated entry point — checks `permissions::domain_allowed(cfg, "docker")`
/// before delegating to `scan` below. `docker` is a structured domain (see
/// `permissions::STRUCTURED_DOMAINS`) but is NOT in `DEFAULT_ENABLED_DOMAINS`,
/// so with zero config anywhere this returns empty — unlike git, a project
/// must explicitly opt in before its Dockerfiles/compose files are read.
pub fn scan_if_allowed(cfg: &crate::permissions::PermissionConfig, root: &Path) -> Vec<Resource> {
    if !crate::permissions::domain_allowed(cfg, "docker") {
        return Vec::new();
    }
    scan(root)
}

/// Every `Resource` this extractor can produce for the repo at `root` — a
/// `docker_image` per `FROM` line found in a root-level `Dockerfile`/
/// `Dockerfile.*`, plus a `docker_service` per service block found in a
/// root-level compose file. Shallow, root-only scan (no recursive walk) —
/// deliberately narrower than `scan.rs`'s tree walk, since a Dockerfile
/// nested arbitrarily deep is a much rarer, much less certain signal than
/// one sitting where `docker build .` / `docker compose up` would actually
/// be run from. UNGATED — see `scan_if_allowed` above for the
/// permission-checked entry point real callers should use instead.
pub fn scan(root: &Path) -> Vec<Resource> {
    let mut resources = Vec::new();
    let repo_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string());

    for (dockerfile_name, dockerfile_path) in dockerfiles_at_root(root) {
        for (idx, image) in from_images(&dockerfile_path).into_iter().enumerate() {
            resources.push(Resource {
                id: Identity(format!("{repo_name}:{dockerfile_name}:{idx}:{image}")),
                kind: "docker_image".to_string(),
                domain: "docker".to_string(),
                location: Location { file: dockerfile_path.display().to_string(), line: None },
                attributes: BTreeMap::from([
                    ("repository".to_string(), repo_name.clone()),
                    ("image".to_string(), image.clone()),
                    ("source".to_string(), dockerfile_name.clone()),
                ]),
                evidence: vec![Evidence {
                    tier: Tier::Declared,
                    what: format!("FROM {image} declared in {dockerfile_name}"),
                }],
            });
        }
    }

    for (compose_name, compose_path) in compose_files_at_root(root) {
        for service in compose_services(&compose_path) {
            let mut attrs = BTreeMap::from([
                ("repository".to_string(), repo_name.clone()),
                ("service".to_string(), service.name.clone()),
                ("source".to_string(), compose_name.clone()),
            ]);
            if let Some(image) = &service.image {
                attrs.insert("image".to_string(), image.clone());
            }
            if !service.ports.is_empty() {
                attrs.insert("ports".to_string(), service.ports.join(","));
            }
            // `build_context`/`build_dockerfile`: what THIS service declares,
            // verbatim, as written — no path resolution, no filesystem check,
            // no identity claim. That's query.rs::built_from_relationships'
            // job (it has the project root; this module doesn't concern
            // itself with cross-domain identity at all, same separation
            // git_domain.rs keeps from same_project_relationships).
            if let Some(context) = &service.build_context {
                attrs.insert("build_context".to_string(), context.clone());
            }
            if let Some(dockerfile) = &service.build_dockerfile {
                attrs.insert("build_dockerfile".to_string(), dockerfile.clone());
            }
            let what = match (&service.image, &service.build_context) {
                (Some(image), Some(context)) => format!(
                    "service '{}' (image '{image}', build context '{context}') declared in {compose_name}",
                    service.name
                ),
                (Some(image), None) => format!(
                    "service '{}' (image '{image}') declared in {compose_name}",
                    service.name
                ),
                (None, Some(context)) => format!(
                    "service '{}' (build context '{context}') declared in {compose_name}",
                    service.name
                ),
                (None, None) => format!("service '{}' declared in {compose_name}", service.name),
            };
            resources.push(Resource {
                id: Identity(format!("{repo_name}:{compose_name}:service:{}", service.name)),
                kind: "docker_service".to_string(),
                domain: "docker".to_string(),
                location: Location { file: compose_path.display().to_string(), line: None },
                attributes: attrs,
                evidence: vec![Evidence { tier: Tier::Declared, what }],
            });
        }
    }

    resources
}

/// The gated LIVE entry point — checks `permissions::domain_allowed(cfg,
/// "docker")`, same gate `scan_if_allowed` uses, then for every root-level
/// compose file, runs `docker compose -f <path> ps --format json --all` and
/// reports each DECLARED service's real, current state as Observed-tier
/// evidence. Never invoked by `scan`/`scan_if_allowed` — see the module doc.
///
/// Silence over a wrong guess applies here exactly like everywhere else in
/// this codebase: if `docker` is not installed, the daemon is unreachable,
/// the command times out, or its output can't be parsed, that compose
/// file's services simply produce NO resources — never a guessed or stale
/// state presented as current. A `docker_service` declared in a compose file
/// this command DID succeed against, but that engine reports has no
/// container at all, is exactly as reportable as one that visibly exited:
/// `--all` makes compose enumerate every declared service's state instead of
/// silently omitting stopped ones, so its absence from that live output is
/// itself observed information, not a gap in the observation.
pub fn scan_observed(cfg: &crate::permissions::PermissionConfig, root: &Path) -> Vec<Resource> {
    if !crate::permissions::domain_allowed(cfg, "docker") {
        return Vec::new();
    }
    let mut resources = Vec::new();
    let repo_name = root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root.display().to_string());

    for (compose_name, compose_path) in compose_files_at_root(root) {
        let declared = compose_services(&compose_path);
        if declared.is_empty() {
            continue;
        }
        let Some(states) = live_compose_states(&compose_path) else { continue };
        for service in declared {
            let (what, running) = match states.get(&service.name) {
                Some(state) if state == "running" => (
                    format!(
                        "service '{}' observed RUNNING right now (docker compose ps, state '{state}')",
                        service.name
                    ),
                    true,
                ),
                Some(state) => (
                    format!(
                        "service '{}' declared in {compose_name} but observed NOT running right now (docker compose ps, state '{state}')",
                        service.name
                    ),
                    false,
                ),
                None => (
                    format!(
                        "service '{}' declared in {compose_name} but observed NOT running right now (docker compose ps --all reports no container for it)",
                        service.name
                    ),
                    false,
                ),
            };
            resources.push(Resource {
                id: Identity(format!(
                    "{repo_name}:{compose_name}:service:{}:observed",
                    service.name
                )),
                kind: "docker_service_observed".to_string(),
                domain: "docker".to_string(),
                location: Location { file: compose_path.display().to_string(), line: None },
                attributes: BTreeMap::from([
                    ("repository".to_string(), repo_name.clone()),
                    ("service".to_string(), service.name.clone()),
                    ("source".to_string(), compose_name.clone()),
                    ("running".to_string(), running.to_string()),
                ]),
                evidence: vec![Evidence { tier: Tier::Observed, what }],
            });
        }
    }

    resources
}

/// Runs `docker compose -f <compose_path> ps --format json --all` with a
/// bounded wait, and parses its JSON-LINES stdout (one JSON object per
/// line — NOT a JSON array; verified against a real `docker compose`
/// invocation before writing this, since guessing the format wrong would
/// silently break every fact this function produces) into a service-name ->
/// state map. Returns `None` on ANY failure — missing binary, nonzero exit,
/// timeout, unparseable output — so the caller can skip this compose file
/// entirely rather than report a partial or stale guess.
fn live_compose_states(compose_path: &Path) -> Option<BTreeMap<String, String>> {
    let mut cmd = Command::new("docker");
    cmd.args(["compose", "-f"])
        .arg(compose_path)
        .args(["ps", "--format", "json", "--all"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let output = run_with_timeout(cmd, Duration::from_secs(10))?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let mut states = BTreeMap::new();
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let (Some(service), Some(state)) =
            (value.get("Service").and_then(|v| v.as_str()), value.get("State").and_then(|v| v.as_str()))
        else {
            continue;
        };
        states.insert(service.to_string(), state.to_string());
    }
    Some(states)
}

/// Spawns `cmd` and waits at most `timeout`, killing and returning `None` on
/// expiry — `Command::output()` alone blocks forever if the docker engine
/// hangs, which a query command must never risk. Polls `try_wait` rather
/// than blocking, since the standard library has no wait-with-timeout.
fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Option<std::process::Output> {
    let mut child = cmd.spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                let output = child.wait_with_output().ok()?;
                return Some(output);
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return None,
        }
    }
}

/// Root-level `Dockerfile` and `Dockerfile.*` (e.g. `Dockerfile.prod`) —
/// matches the common convention of one primary Dockerfile plus optional
/// environment-specific variants living alongside it, without recursing.
fn dockerfiles_at_root(root: &Path) -> Vec<(String, std::path::PathBuf)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else { return out };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else { continue };
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "Dockerfile" || name.starts_with("Dockerfile.") {
            out.push((name, entry.path()));
        }
    }
    out.sort();
    out
}

/// Every `FROM <image>` base image in a Dockerfile, in declaration order,
/// including multi-stage builds (`FROM node:20 AS build`) — the alias after
/// `AS` is dropped, only the image reference itself is kept as evidence.
/// `FROM scratch` is included too: it is a real, meaningful declaration
/// (the DECLARED fact is "this image is the base," not "this image is
/// interesting"), not a special case to filter out.
fn from_images(dockerfile: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dockerfile) else { return Vec::new() };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("FROM ").or_else(|| line.strip_prefix("from ")) else {
            continue;
        };
        let image = rest.split_whitespace().next().unwrap_or("").to_string();
        if !image.is_empty() {
            out.push(image);
        }
    }
    out
}

/// Root-level compose files, most-conventional-name first, but ALL found
/// names are scanned (a repo with both `docker-compose.yml` and an override
/// file both declare real services) — matches the git domain's own "read
/// what's declared, don't guess which single file is authoritative"
/// posture.
fn compose_files_at_root(root: &Path) -> Vec<(String, std::path::PathBuf)> {
    let candidates =
        ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml"];
    candidates
        .iter()
        .filter_map(|name| {
            let path = root.join(name);
            path.is_file().then(|| (name.to_string(), path))
        })
        .collect()
}

struct ComposeService {
    name: String,
    image: Option<String>,
    ports: Vec<String>,
    /// `build.context` — either the shorthand scalar form (`build: ./dir`)
    /// or the block form's `context:` key. Verbatim, as written — see this
    /// module's own doc for why no YAML-aware resolution happens here.
    build_context: Option<String>,
    /// `build.dockerfile`, block form only. Captured alongside `context`
    /// since it's free to parse once already inside the `build:` block, but
    /// not required by anything that consumes it yet.
    build_dockerfile: Option<String>,
}

/// Hand-parses the common, simple block-style subset of a compose file's
/// top-level `services:` mapping — see this module's doc comment for why a
/// full YAML parser isn't used here. Indentation-based: a `services:` line
/// at column 0, each service name as a key one level in, each service's own
/// `image:`/`ports:` keys one level deeper still. Any line this parser
/// doesn't recognize is skipped, not treated as an error — a compose file
/// using YAML features beyond this subset simply yields fewer resources.
fn compose_services(compose_path: &Path) -> Vec<ComposeService> {
    let Ok(text) = std::fs::read_to_string(compose_path) else { return Vec::new() };
    let lines: Vec<&str> = text.lines().collect();

    // Find the `services:` top-level key and its indentation (almost always
    // column 0, but tolerate a globally-indented file rather than assume).
    let Some(services_idx) = lines.iter().position(|l| l.trim_end() == "services:"
        || l.trim_start() == "services:" && indent_of(l) == 0)
    else {
        return Vec::new();
    };
    let services_indent = indent_of(lines[services_idx]);

    let mut services = Vec::new();
    let mut i = services_idx + 1;
    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            i += 1;
            continue;
        }
        let this_indent = indent_of(line);
        if this_indent <= services_indent {
            break; // dedented back out of the services: block entirely
        }
        // A service name line: exactly one level deeper than `services:`,
        // ending in `:` (with nothing else on the line — block-style key).
        let service_indent = this_indent;
        let trimmed = line.trim();
        if let Some(name) = trimmed.strip_suffix(':') {
            if !name.is_empty() && !name.contains(' ') {
                let mut image = None;
                let mut ports = Vec::new();
                let mut build_context = None;
                let mut build_dockerfile = None;
                let mut j = i + 1;
                let mut in_ports = false;
                let mut in_build = false;
                while j < lines.len() {
                    let inner = lines[j];
                    if inner.trim().is_empty() {
                        j += 1;
                        continue;
                    }
                    let inner_indent = indent_of(inner);
                    if inner_indent <= service_indent {
                        break; // dedented back out of this service's block
                    }
                    let inner_trimmed = inner.trim();
                    if let Some(val) = inner_trimmed.strip_prefix("image:") {
                        image = Some(strip_yaml_quotes(val.trim()));
                        in_ports = false;
                        in_build = false;
                    } else if inner_trimmed == "ports:" {
                        in_ports = true;
                        in_build = false;
                    } else if let Some(item) = inner_trimmed.strip_prefix("- ") {
                        if in_ports {
                            ports.push(strip_yaml_quotes(item.trim()));
                        }
                    } else if in_build && inner_trimmed.strip_prefix("context:").is_some() {
                        let val = inner_trimmed.strip_prefix("context:").unwrap().trim();
                        if !val.is_empty() {
                            build_context = Some(strip_yaml_quotes(val));
                        }
                    } else if in_build && inner_trimmed.strip_prefix("dockerfile:").is_some() {
                        let val = inner_trimmed.strip_prefix("dockerfile:").unwrap().trim();
                        if !val.is_empty() {
                            build_dockerfile = Some(strip_yaml_quotes(val));
                        }
                    } else if inner_trimmed == "build:" {
                        // Block form: `context:`/`dockerfile:` follow, nested
                        // one level deeper — matches `ports:`'s own list-item
                        // pattern above, same flat-state-machine style.
                        in_build = true;
                        in_ports = false;
                    } else if let Some(val) = inner_trimmed.strip_prefix("build:") {
                        // Shorthand scalar form: `build: ./some/dir` IS the
                        // context directly — compose spec allows this as an
                        // alternative to the block form.
                        let val = val.trim();
                        if !val.is_empty() {
                            build_context = Some(strip_yaml_quotes(val));
                        }
                        in_ports = false;
                        in_build = false;
                    } else if inner_trimmed.ends_with(':') {
                        in_ports = false; // moved to some other key we don't parse
                        in_build = false;
                    }
                    j += 1;
                }
                services.push(ComposeService {
                    name: name.to_string(),
                    image,
                    ports,
                    build_context,
                    build_dockerfile,
                });
                i = j;
                continue;
            }
        }
        i += 1;
    }
    services
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn strip_yaml_quotes(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_project(label: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir()
            .join(format!("archietect-docker-domain-test-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn scan_finds_dockerfile_base_image() {
        let root = tmp_project("dockerfile");
        std::fs::write(
            root.join("Dockerfile"),
            "FROM node:20-alpine AS build\nWORKDIR /app\nCOPY . .\n",
        )
        .unwrap();

        let resources = scan(&root);
        let image = resources.iter().find(|r| r.kind == "docker_image");
        assert!(image.is_some(), "expected a docker_image resource");
        let image = image.unwrap();
        assert_eq!(image.domain, "docker");
        assert_eq!(image.attributes.get("image").map(String::as_str), Some("node:20-alpine"));
        assert_eq!(image.evidence[0].tier, Tier::Declared);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_finds_compose_service_with_image_and_ports() {
        let root = tmp_project("compose");
        std::fs::write(
            root.join("docker-compose.yml"),
            "version: \"3\"\nservices:\n  web:\n    image: myapp:latest\n    ports:\n      - \"3000:3000\"\n      - \"3001:3001\"\n  redis:\n    image: redis:7\n",
        )
        .unwrap();

        let resources = scan(&root);
        let services: Vec<_> = resources.iter().filter(|r| r.kind == "docker_service").collect();
        assert_eq!(services.len(), 2, "expected two services, got: {services:?}");

        let web = services
            .iter()
            .find(|r| r.attributes.get("service").map(String::as_str) == Some("web"))
            .expect("expected a 'web' service resource");
        assert_eq!(web.attributes.get("image").map(String::as_str), Some("myapp:latest"));
        assert_eq!(web.attributes.get("ports").map(String::as_str), Some("3000:3000,3001:3001"));
        assert_eq!(web.evidence[0].tier, Tier::Declared);

        let redis = services
            .iter()
            .find(|r| r.attributes.get("service").map(String::as_str) == Some("redis"))
            .expect("expected a 'redis' service resource");
        assert_eq!(redis.attributes.get("image").map(String::as_str), Some("redis:7"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_on_project_with_no_docker_files_returns_empty() {
        let root = tmp_project("none");
        assert!(scan(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_if_allowed_blocks_by_default_with_zero_config() {
        use crate::permissions::PermissionConfig;
        let root = tmp_project("default-disabled");
        std::fs::write(root.join("Dockerfile"), "FROM alpine\n").unwrap();

        let cfg = PermissionConfig::default();
        assert!(
            scan_if_allowed(&cfg, &root).is_empty(),
            "docker must default to disabled with zero config present, unlike git"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_if_allowed_permits_when_explicitly_enabled() {
        let root = tmp_project("explicit-enabled");
        std::fs::write(root.join("Dockerfile"), "FROM alpine\n").unwrap();
        std::fs::write(root.join("archietect.toml"), "[domains]\ndocker = \"enabled\"\n").unwrap();

        let cfg = crate::permissions::load(Path::new("/nonexistent/global.toml"), &root).unwrap();
        assert!(
            !scan_if_allowed(&cfg, &root).is_empty(),
            "an explicit project-level enable must permit scanning"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_captures_build_context_block_form_and_shorthand() {
        let root = tmp_project("build-context");
        std::fs::write(
            root.join("docker-compose.yml"),
            "services:\n  api:\n    build:\n      context: ./backend\n      dockerfile: Dockerfile.prod\n  worker:\n    build: ./worker-src\n  cache:\n    image: redis:7\n",
        )
        .unwrap();

        let resources = scan(&root);
        let services: Vec<_> = resources.iter().filter(|r| r.kind == "docker_service").collect();
        assert_eq!(services.len(), 3, "expected three services, got: {services:?}");

        let api = services.iter().find(|r| r.attributes.get("service").map(String::as_str) == Some("api")).unwrap();
        assert_eq!(api.attributes.get("build_context").map(String::as_str), Some("./backend"));
        assert_eq!(api.attributes.get("build_dockerfile").map(String::as_str), Some("Dockerfile.prod"));

        let worker = services.iter().find(|r| r.attributes.get("service").map(String::as_str) == Some("worker")).unwrap();
        assert_eq!(worker.attributes.get("build_context").map(String::as_str), Some("./worker-src"), "shorthand `build: ./dir` form must be captured as the context");

        let cache = services.iter().find(|r| r.attributes.get("service").map(String::as_str) == Some("cache")).unwrap();
        assert!(cache.attributes.get("build_context").is_none(), "a plain image: service must have no build_context");

        let _ = std::fs::remove_dir_all(&root);
    }

    fn docker_enabled_cfg(root: &Path) -> crate::permissions::PermissionConfig {
        std::fs::write(root.join("archietect.toml"), "[domains]\ndocker = \"enabled\"\n").unwrap();
        crate::permissions::load(Path::new("/nonexistent/global.toml"), root).unwrap()
    }

    #[test]
    fn scan_observed_blocked_by_default_with_zero_config() {
        let root = tmp_project("observed-default-disabled");
        std::fs::write(
            root.join("docker-compose.yml"),
            "services:\n  web:\n    image: nginx:alpine\n",
        )
        .unwrap();

        let cfg = crate::permissions::PermissionConfig::default();
        assert!(
            scan_observed(&cfg, &root).is_empty(),
            "docker observe must respect the same default-disabled gate as the declarative scan"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_observed_returns_empty_for_compose_file_with_no_declared_services() {
        let root = tmp_project("observed-no-services");
        std::fs::write(root.join("docker-compose.yml"), "version: \"3\"\n").unwrap();
        let cfg = docker_enabled_cfg(&root);

        assert!(
            scan_observed(&cfg, &root).is_empty(),
            "a compose file declaring no services must never trigger a live docker call"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn live_compose_states_returns_none_for_a_compose_file_docker_cannot_run_against() {
        // Real `docker` binary, real invocation, against a path that does
        // not exist — the exact failure this project verified live before
        // writing scan_observed (exit 1, "no such file or directory").
        // Exercises the same graceful-None path a missing binary or an
        // unreachable daemon would also take.
        let result = live_compose_states(Path::new("/nonexistent/archietect-test/compose.yml"));
        assert!(result.is_none(), "a compose file docker can't open must yield None, never a guessed state");
    }

    /// Kills and removes the real containers/network this test brings up,
    /// even on assertion failure — a leaked `nginx:alpine`/`redis:7-alpine`
    /// pair left running would silently pollute every later docker test.
    struct ComposeGuard(std::path::PathBuf);
    impl Drop for ComposeGuard {
        fn drop(&mut self) {
            let _ = Command::new("docker")
                .args(["compose", "-f"])
                .arg(&self.0)
                .args(["down", "--timeout", "1"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }

    #[test]
    fn scan_observed_reports_real_running_and_stopped_services_via_real_docker_compose() {
        // Real `docker compose up` + real `docker compose ps` — not
        // synthetic JSON. Requires docker with `nginx:alpine` and
        // `redis:7-alpine` already pulled locally (both confirmed present
        // in this sandbox); a machine without docker or those images simply
        // fails this one test; every other test in this module is
        // unaffected.
        let root = tmp_project("observed-live");
        let compose_path = root.join("docker-compose.yml");
        std::fs::write(
            &compose_path,
            "services:\n  web:\n    image: nginx:alpine\n  cache:\n    image: redis:7-alpine\n",
        )
        .unwrap();
        let cfg = docker_enabled_cfg(&root);

        let up = Command::new("docker")
            .args(["compose", "-f"])
            .arg(&compose_path)
            .args(["up", "-d", "web"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _guard = ComposeGuard(compose_path.clone());
        assert!(up.map(|s| s.success()).unwrap_or(false), "docker compose up -d web must succeed in this sandbox");

        let resources = scan_observed(&cfg, &root);
        let observed: Vec<_> = resources.iter().filter(|r| r.kind == "docker_service_observed").collect();
        assert_eq!(observed.len(), 2, "expected one observed resource per declared service, got: {observed:?}");

        let web = observed.iter().find(|r| r.attributes.get("service").map(String::as_str) == Some("web")).unwrap();
        assert_eq!(web.attributes.get("running").map(String::as_str), Some("true"));
        assert_eq!(web.evidence[0].tier, Tier::Observed);
        assert!(web.evidence[0].what.contains("RUNNING"), "{}", web.evidence[0].what);

        let cache = observed.iter().find(|r| r.attributes.get("service").map(String::as_str) == Some("cache")).unwrap();
        assert_eq!(
            cache.attributes.get("running").map(String::as_str),
            Some("false"),
            "cache' was declared but never started — must be observed NOT running, never omitted or guessed"
        );
        assert!(cache.evidence[0].what.contains("NOT running"), "{}", cache.evidence[0].what);

        drop(_guard);
        let _ = std::fs::remove_dir_all(&root);
    }
}
