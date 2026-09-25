//! Explicit, read-only runtime probes.
//!
//! Static extraction cannot establish that an HTTP route is reachable. This
//! module provides a deliberately small probe for plain HTTP targets. It does
//! not follow redirects, send credentials, execute JavaScript, or write to the
//! architectural index.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_TIMEOUT_MS: u64 = 5_000;
const MAX_TIMEOUT_MS: u64 = 30_000;
const MAX_BODY_BYTES: usize = 1_048_576;
const MAX_BROWSER_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_BROWSER_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_BROWSER_SETTLE_MS: u64 = 250;
const MAX_BROWSER_SETTLE_MS: u64 = 3_000;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    host: String,
    port: u16,
    path: String,
}

fn parse_target(raw: &str) -> Result<Target> {
    let url = raw.trim();
    let rest = url.strip_prefix("http://").ok_or_else(|| anyhow!("only http:// targets are supported; HTTPS requires a TLS-capable client and is not probed by this command"))?;
    if rest.is_empty() || rest.starts_with('/') {
        bail!("target must include a host")
    }
    let (authority, path) = rest.split_once('/').map_or((rest, "/"), |(a, p)| (a, p));
    if authority.contains('@') || authority.contains('?') || authority.contains('#') {
        bail!("credentials and fragments are not accepted in runtime probe targets")
    }
    let (host, port) = if let Some((host, port)) = authority.rsplit_once(':') {
        if host.is_empty() || host.contains(':') {
            bail!("invalid host/port")
        }
        (
            host.to_string(),
            port.parse::<u16>().context("invalid port")?,
        )
    } else {
        (authority.to_string(), 80)
    };
    if host.is_empty() || path.contains(' ') {
        bail!("invalid target")
    }
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    Ok(Target { host, port, path })
}

fn extract_title(body: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(body);
    let lower = text.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = lower[start..].find('>')? + start + 1;
    let end = lower[open_end..].find("</title>")? + open_end;
    let title = text[open_end..end].trim().to_string();
    (!title.is_empty()).then_some(title)
}

/// Perform one bounded GET and return runtime evidence. No redirects,
/// authentication, cookies, JavaScript, or writes are involved.
pub fn verify_http(raw_url: &str, timeout_ms: Option<u64>) -> Result<Value> {
    let target = parse_target(raw_url)?;
    let timeout_ms = timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1, MAX_TIMEOUT_MS);
    let timeout = Duration::from_millis(timeout_ms);
    let mut addrs = (target.host.as_str(), target.port)
        .to_socket_addrs()
        .with_context(|| format!("could not resolve {}", target.host))?;
    let addr = addrs.next().ok_or_else(|| anyhow!("no address resolved"))?;
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .with_context(|| format!("could not connect to {raw_url}"))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: archietect-runtime-probe/{}\r\nAccept: text/html,application/json;q=0.9,*/*;q=0.1\r\n\r\n",
        target.path, target.host, env!("CARGO_PKG_VERSION")
    );
    stream.write_all(request.as_bytes())?;
    let mut response = Vec::new();
    let mut buf = [0_u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                response.extend_from_slice(&buf[..n]);
                if response.len() > MAX_BODY_BYTES + 32_768 {
                    response.truncate(MAX_BODY_BYTES + 32_768);
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => break,
            Err(e) => return Err(e.into()),
        }
    }
    let header_end = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow!("response did not contain HTTP headers"))?;
    let header_text = String::from_utf8_lossy(&response[..header_end]);
    let mut lines = header_text.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| anyhow!("response had no status line"))?;
    let mut status_parts = status_line.splitn(3, ' ');
    let protocol = status_parts.next().unwrap_or("");
    let status = status_parts
        .next()
        .ok_or_else(|| anyhow!("malformed status line"))?
        .parse::<u16>()
        .context("malformed status code")?;
    if !protocol.starts_with("HTTP/") {
        bail!("unsupported response protocol")
    }
    let mut headers = serde_json::Map::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            if matches!(
                name.as_str(),
                "content-type" | "location" | "server" | "content-length" | "x-request-id"
            ) {
                headers.insert(name, json!(value.trim()));
            }
        }
    }
    let body = &response[(header_end + 4)..];
    let body = &body[..body.len().min(MAX_BODY_BYTES)];
    Ok(json!({
        "evidence": "RUNTIME",
        "kind": "http_response",
        "verdict": if status >= 400 { "CONTRADICTED" } else { "VERIFIED" },
        "url": raw_url,
        "host": target.host,
        "port": target.port,
        "path": target.path,
        "observed_at_ms": started,
        "timeout_ms": timeout_ms,
        "status": status,
        "ok": (200..400).contains(&status),
        "headers": headers,
        "body_bytes": body.len(),
        "title": extract_title(body),
        "redirect_followed": false,
        "credentials_sent": false,
        "javascript_executed": false,
        "note": "One read-only HTTP GET; this is network evidence, not proof that a browser-rendered page hydrates or that authenticated flows work."
    }))
}

/// Convert a transport-level failure into explicit runtime evidence. A failed
/// connection is not proof that a statically declared route is absent; it is
/// an unverified observation with the error preserved for investigation.
pub fn http_error_evidence(raw_url: &str, error: &anyhow::Error) -> Value {
    json!({
        "evidence": "RUNTIME",
        "kind": "http_probe_error",
        "verdict": "UNVERIFIED",
        "url": raw_url,
        "error": error.to_string(),
        "note": "The runtime probe could not establish an HTTP observation; this must not be interpreted as route absence."
    })
}

/// Run a bounded, read-only browser check through an already-installed
/// Playwright package. This intentionally uses a subprocess rather than a
/// Rust browser binding: Archietect has no browser runtime dependency and the
/// target project's Playwright installation is the source of truth.
///
/// No storage state, cookies, credentials, or headed browser are used. The
/// result is browser evidence, not a security or authenticated-flow result.
pub fn verify_browser(
    raw_url: &str,
    timeout_ms: Option<u64>,
    settle_ms: Option<u64>,
) -> Result<Value> {
    validate_browser_url(raw_url)?;
    let timeout_ms = timeout_ms
        .unwrap_or(DEFAULT_BROWSER_TIMEOUT_MS)
        .clamp(1, MAX_BROWSER_TIMEOUT_MS);
    let settle_ms = settle_ms
        .unwrap_or(DEFAULT_BROWSER_SETTLE_MS)
        .clamp(0, MAX_BROWSER_SETTLE_MS);
    let script = r#"
const { chromium } = require("playwright");
(async () => {
  const url = process.argv[1];
  const timeout = Number(process.argv[2]);
  const settle = Number(process.argv[3]);
  const consoleMessages = [];
  const pageErrors = [];
  const requestFailures = [];
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage({
      javaScriptEnabled: true,
      serviceWorkers: "block",
    });
    page.on("console", message => {
      if (["error", "warning"].includes(message.type()) && consoleMessages.length < 50) {
        consoleMessages.push({ type: message.type(), text: message.text().slice(0, 500) });
      }
    });
    page.on("pageerror", error => {
      if (pageErrors.length < 25) pageErrors.push(String(error).slice(0, 500));
    });
    page.on("requestfailed", request => {
      if (requestFailures.length < 50) {
        requestFailures.push({ url: request.url().slice(0, 500), error: request.failure()?.errorText || "unknown" });
      }
    });
    const response = await page.goto(url, { waitUntil: "domcontentloaded", timeout });
    if (settle > 0) await page.waitForTimeout(settle);
    const accessibility = await page.evaluate(() => {
      const text = (element) => (element.getAttribute("aria-label") || element.textContent || "").trim();
      const images = Array.from(document.images);
      const buttons = Array.from(document.querySelectorAll("button"));
      const links = Array.from(document.querySelectorAll("a"));
      return {
        htmlLang: document.documentElement.getAttribute("lang"),
        landmarkCount: document.querySelectorAll("main, nav, header, footer, aside").length,
        imageCount: images.length,
        imagesMissingAlt: images.filter(image => !image.hasAttribute("alt")).length,
        buttonCount: buttons.length,
        buttonsMissingName: buttons.filter(button => !text(button)).length,
        linkCount: links.length,
        linksMissingHref: links.filter(link => !link.getAttribute("href")).length,
      };
    });
    console.log(JSON.stringify({
      evidence: "BROWSER",
      kind: "browser_page",
      url,
      status: response ? response.status() : null,
      ok: Boolean(response && response.status() >= 200 && response.status() < 400),
      title: await page.title(),
      finalUrl: page.url(),
      consoleMessages,
      pageErrors,
      requestFailures,
      accessibility,
      javascriptExecuted: true,
      credentialsSent: false,
      storageStateUsed: false,
      timeoutMs: timeout,
      settleMs: settle,
      note: "Headless Playwright navigation with basic DOM/accessibility checks; no credentials or storage state. This is not proof of authenticated behaviour, security, or visual correctness."
    }));
  } finally {
    await browser.close();
  }
})().catch(error => {
  console.error(String(error && error.stack || error));
  process.exitCode = 1;
});
"#;
    let mut child = Command::new(
        std::env::var_os("ARCHIETECT_NODE").unwrap_or_else(|| "node".into()),
    )
        .arg("-e")
        .arg(script)
        .arg("--")
        .arg(raw_url)
        .arg(timeout_ms.to_string())
        .arg(settle_ms.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start node; install Node.js and the target project's Playwright package")?;
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms + 5_000);
    loop {
        if let Some(status) = child.try_wait()? {
            let output = child.wait_with_output()?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            if status.success() {
                let line = stdout.lines().last().ok_or_else(|| anyhow!("browser verifier returned no JSON"))?;
                return serde_json::from_str(line).context("browser verifier returned invalid JSON");
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("browser verifier failed: {}", stderr.trim());
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("browser verifier exceeded {}ms", timeout_ms + 5_000);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn validate_browser_url(raw_url: &str) -> Result<()> {
    let url = raw_url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        bail!("browser target must use http:// or https://")
    }
    let authority = url
        .split_once("://")
        .and_then(|(_, rest)| rest.split('/').next())
        .unwrap_or("");
    if authority.is_empty() || authority.contains('@') || url.contains('\n') || url.contains('\r') {
        bail!("browser target must include a host and cannot contain credentials or control characters")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_targets_and_extracts_titles() {
        assert!(parse_target("https://example.test").is_err());
        assert!(parse_target("http://user:pass@example.test").is_err());
        let target = parse_target("http://127.0.0.1:3000/health").unwrap();
        assert_eq!(target.path, "/health");
        assert_eq!(target.port, 3000);
        assert_eq!(
            extract_title(b"<html><title>Runtime test</title></html>"),
            Some("Runtime test".to_string())
        );
        assert!(validate_browser_url("http://127.0.0.1:3000").is_ok());
        assert!(validate_browser_url("https://example.test/path").is_ok());
        assert!(validate_browser_url("ftp://example.test").is_err());
        assert!(validate_browser_url("http://user:pass@example.test").is_err());
    }
}
