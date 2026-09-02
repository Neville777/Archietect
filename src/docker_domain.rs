//! Second structured domain, added to test whether `resource::Resource`
//! generalizes past git — see SYSTEM_MEMORY.md's rollout, phase 3 (git) was
//! the first non-code domain; this is the second. Mirrors `git_domain.rs`'s
//! shape exactly: a raw `scan()` primitive, a gated `scan_if_allowed()` entry
//! point, module doc justifying each tier choice instead of asserting it.
//!
//! Static, declarative parsing only — no `docker` CLI invocation, no daemon
//! socket connection, same restraint `git_domain.rs` shows by reading
//! plaintext files instead of shelling out to `git`. This means every
//! `Resource` here is DECLARED, never OBSERVED: this domain cannot see
//! whether a container is actually running, only what a Dockerfile or
//! compose file asserts should exist. Checking real container state would
//! require talking to a live daemon, which is out of scope for the same
//! reason `git_domain.rs` never shells out to `git status`/`git branch -a`
//! for anything beyond reading HEAD once.
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
            let what = match &service.image {
                Some(image) => format!(
                    "service '{}' (image '{image}') declared in {compose_name}",
                    service.name
                ),
                None => format!("service '{}' declared in {compose_name}", service.name),
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
                let mut j = i + 1;
                let mut in_ports = false;
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
                    } else if inner_trimmed == "ports:" {
                        in_ports = true;
                    } else if let Some(item) = inner_trimmed.strip_prefix("- ") {
                        if in_ports {
                            ports.push(strip_yaml_quotes(item.trim()));
                        }
                    } else if inner_trimmed.ends_with(':') {
                        in_ports = false; // moved to some other key we don't parse
                    }
                    j += 1;
                }
                services.push(ComposeService { name: name.to_string(), image, ports });
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
}
