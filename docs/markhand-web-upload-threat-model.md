# Markhand Web upload threat model (P0-09)

Status: Done for local policy/sandbox smoke evidence. This model is a Phase 0
control specification for the upload and conversion path; it is not malware
scanner evidence and does not claim Profile B production sandbox hardening.

## Scope

In scope:

- Browser/API upload intake, quarantine, object-key materialization, and worker
  dispatch.
- Converter worker execution for pdf, docx, pptx, xlsx, csv, html, text, image
  OCR, and audio transcription.
- Runtime model/native dependency decisions that affect upload conversion.
- Cross-tenant, quota, token, and compromised-worker failure modes.

Out of scope for P0-09:

- Full antivirus/malware signature scanning.
- VM or hardware-enforced sandbox escape resistance.
- Production capacity proof on Profile B hardware.

## Trust boundaries

1. User-controlled browser and filenames are untrusted.
2. Upload API authenticates the user and resolves tenant/project membership.
3. Quarantine object storage stores untrusted bytes before parser execution.
4. Worker sandbox reads quarantined bytes and writes only server-derived output.
5. Indexing and retrieval consume converted Markdown as untrusted content.
6. LLM/cloud providers are only reachable from explicitly approved services, not
   from upload conversion workers.

## Threat table

| Threat | Example | Prevention controls | Detection/evidence | Owner |
|---|---|---|---|---|
| MIME/extension spoof | Plain text named `.pdf` or HTML named `.pdf` | Extension and magic-byte canonicalization; allowlist only; reject mismatches before conversion | Adversarial fixtures `adv-spoof-pdf`, `adv-html-pdf`; upload rejection audit | security-owner |
| Archive bomb | OOXML/zip with extreme compression ratio or entry count | Pre-scan central directory; max entries, uncompressed bytes, and compression ratio; no recursive archive expansion | `adv-zip-bomb`; sandbox resource-limit event | worker-owner |
| Parser corruption/exploit | Truncated PDF or malformed OOXML | Quarantine first; parser runs only in sandbox; fail closed on parser exceptions; patched native runtimes from inventory | `adv-corrupt-pdf`, `adv-malformed-docx`; converter error disposition | worker-owner |
| SSRF and unexpected network | HTML/PDF references remote URL or worker library opens network | Upload worker has no egress, no DNS, no loopback; HTML sanitizer strips active content; no remote fetch in converter | Harness egress-denial simulation; sandbox denied audit | security-owner |
| Resource exhaustion | Page bombs, huge images, long audio, high memory OCR | Byte/page/pixel/audio-duration limits; CPU/RAM/file/process/wall-clock limits; process-group kill | `adv-page-bomb`, `adv-long-audio`; timeout/resource-limit metrics | worker-owner |
| Path traversal | Archive member `../secret` or absolute paths | Reject absolute paths, `..`, drive prefixes, symlinks, and host path mounts; server-generated output keys only | `adv-traversal-docx`; traversal-denial simulation | security-owner |
| Content/prompt injection | HTML says "ignore previous instructions" | Converted content is stored as untrusted passage text; prompt policy frames passages; no tools or secrets exposed to retrieved content | `adv-prompt-html`; quarantine disposition and retrieval prompt tests | retrieval-owner |
| CSV/formula injection | Spreadsheet formula begins with `=`, `+`, `-`, or `@` | Quarantine or escape formula-leading cells before export/display; never execute formulas during conversion | `adv-formula-csv`; formula disposition in adversarial map | application-owner |
| Token theft | Uploaded content tries to read API keys or worker env | Worker receives no user/provider tokens; no secret mounts; redacted logs; no egress | Secret scan on gate registry; sandbox env allowlist review | security-owner |
| Quota race | Many concurrent uploads exceed tenant budget before counters settle | Reserve quota before object acceptance; idempotency key; atomic decrement on failure; per-tenant queue caps | Upload quota metrics and rejected-over-quota audit | platform-owner |
| Tenant isolation break | User uploads into another tenant or output key collides | Tenant/project authorization before presign; server-derived object keys include tenant/version namespace; RLS for metadata | Access-denied audit and object-key validation tests | platform-owner |
| Compromised worker | Parser exploit gains code execution in converter | Non-root, read-only root, no egress, dropped capabilities, tmpfs workdir, cgroup/seccomp limits, short-lived worker, no credentials | Sandbox profile validation; worker heartbeat and unexpected-exit audit | security-owner |

## Required security invariants

- Uploaded filenames are metadata only and are never used as filesystem paths or
  object keys.
- No parser sees bytes until the upload API has authenticated the user, reserved
  quota, verified the allowlist, and stored the object in quarantine.
- Conversion workers are disposable and run with no user tokens, no provider
  tokens, and no network egress.
- Unresolved or unknown model licenses are excluded from runtime bundles.
- Converted text remains untrusted until prompt framing, citation validation, and
  tenant authorization checks complete downstream.

## P0-09 evidence

- Upload policy: `docs/markhand-web-upload-policy.md`
- Sandbox profile: `bench/markhand_web/security/sandbox-profile.json`
- Adversarial disposition map:
  `bench/markhand_web/security/adversarial-disposition.json`
- Harness and summary:
  `bench/markhand_web/scripts/run_upload_security.py`,
  `bench/markhand_web/security/summary.json`
- Runtime license inventory:
  `docs/markhand-web-runtime-license-inventory.json`
