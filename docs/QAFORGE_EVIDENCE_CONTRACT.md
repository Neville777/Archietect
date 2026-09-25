# QAForge evidence contract

Archietect can emit runtime observations in a stable envelope for QAForge or
another evidence store. The contract is versioned independently from the
Archietect binary:

```text
qaforge.evidence.v1
```

## Producing evidence

Use `--qaforge` on a runtime probe:

```bash
archietect runtime verify-http \
  --url http://127.0.0.1:3000/health \
  --qaforge > evidence.json

archietect runtime verify-browser \
  --url http://127.0.0.1:3000 \
  --qaforge > evidence.json
```

The MCP tools `runtime_verify_http` and `runtime_verify_browser` accept the
same `qaforge: true` argument. This makes the contract available to an agent
without requiring the agent to parse CLI output.

## Envelope

Every exported result has this shape:

```json
{
  "contract_version": "qaforge.evidence.v1",
  "evidence_type": "http_response",
  "source": "archietect.runtime.verify_http",
  "observed_at_ms": 1770000000000,
  "commit": "abc123...",
  "target": { "url": "http://127.0.0.1:3000/health" },
  "confidence": "high",
  "limitations": [
    "One read-only HTTP GET; this is network evidence, not proof that a browser-rendered page hydrates or that authenticated flows work."
  ],
  "payload": {}
}
```

`commit` is the repository `HEAD` observed for the supplied/discovered root;
it is `null` when the target is not inside a Git repository. A consumer must
not infer that `null` means the evidence is current.

`evidence_type` currently includes `http_response` and `browser_page`.
`payload` retains the complete probe-specific result, including status,
console/page errors, failed requests, and accessibility counts where
applicable. `limitations` is intentionally explicit: this contract records
what was observed, not what the probe did not test.

## QAForge ingestion rules

1. Validate `contract_version` before persistence.
2. Store `source`, `observed_at_ms`, `commit`, `target`, `confidence`, and
   `limitations` as first-class evidence metadata.
3. Store `payload` as the adapter-specific evidence body.
4. Do not promote `confidence` to a security severity. A high-confidence HTTP
   response still does not prove authentication, authorization, or browser
   correctness.
5. Correlate with a QAForge `Run` using the target URL, project, commit, and
   ingestion timestamp; do not use a URL alone as an identity.
6. Preserve the original envelope for auditability. Do not rewrite the source
   payload during normalization.

The Rust library also exposes `evidence::validate_envelope` for an adapter or
integration test that wants to reject malformed envelopes before persistence.
