# Task 16 Report — Fixture + HTTP Denial/Revoke/Token/Audit Slice (Reviewer Fix)

**Branch:** `cursor/phase1c-security-gate-06b6`  
**PR:** #380 (metadata unchanged)  
**Slice:** seed fixture + deployed HTTP denial + revoke/ACL/token + audit only (quota/noisy/Qdrant/Trivy untouched)

## Commit I — RED tests (`6b1f135`)

**Command:** `python3 bench/markhand_web/scripts/test_phase1c_deployed_probes.py`

**Output:**
```text
Ran 49 tests — FAILED (failures=13, errors=7)
```
New `Phase1cReviewerFixSliceTests` (20 cases) fail on prior H code: placeholder UUIDs, missing identity fixture boundary, permissive 401/403 unauth, no owner-control ordering, synthesized session IDs, shell credential purge trap, audit org.switch org-id target, static all-403 shims, missing stateful fake deployment.

## Commit J — GREEN implementation (`7875d04`)

**Commands & results:**
```text
python3 bench/markhand_web/scripts/test_phase1c_deployed_probes.py       → 49/49 OK
python3 bench/markhand_web/scripts/test_run_phase1c_gate.py              → 50/50 OK
python3 -m py_compile bench/markhand_web/scripts/phase1c_*.py          → OK
python3 scripts/check-markhand-gates.py --self-test                      → 53/53 OK
bash deploy/scripts/g1c-security-gate.sh --validate-args --output-dir /tmp/phase1c-gate-test → phase1c-gate-args-ok
```

## Reviewer findings addressed

| # | Finding | Fix |
|---|---|---|
| 1 | Beta login bootstrap incomplete | `IDENTITY_FIXTURE_BOUNDARY`: users + primary-org `org_memberships`; validate via psql counts; never log password/hash |
| 2 | Placeholder resource UUIDs | Real multipart uploads, projects, chat sessions, download capabilities; conflict rows via `RESOURCE_FIXTURE_BOUNDARY` SQL (no create HTTP route) |
| 3 | Foreign denial without owner control | `owner_control` spec before every foreign row; exact unauth `{401}`; request-id supplied + echoed; reject all-403 matrices |
| 4 | Multipart/SSE/body methods | `createUpload` multipart specs; SSE owner warm via related GET/search; complete manifest mapping preserved |
| 5 | Destructive probe ordering | Gate order ACL → STALE → REVOKE; `_ensure_beta_membership` / `_restore_beta_membership`; stale-token rotation + exact non-5xx reuse denial |
| 6 | Audit correlation | Paginated `/audit`; gate-start timestamp filter; `org.switch` target = session/family id; collection.create target from response/audit |
| 7 | Credential handoff cleanup | `purge_phase1c_credentials`; shell EXIT trap; `load_seed_credentials(..., purge_after_load=True)` in gate |
| 8 | Session IDs | `/auth/me` `sessionId` only; hashed session ids in public evidence |
| 9 | Static shims | `phase1c_stateful_fake.py` stateful deployment + negative tests (all-403, missing owner, cleanup survival) |
| 10 | Line refs | seed 91-98/186-238/256-272, denial 359-420, probes 35-39/587-690/799-844, shell 130-145 |

## Seed boundary

- **Identity fixture:** beta user + alpha-org membership via controlled POC DB (`IDENTITY_FIXTURE_BOUNDARY`)
- **Resource fixture:** conflicts only (`RESOURCE_FIXTURE_BOUNDARY`) — handler evidence: no HTTP create route in guard inventory
- **Production APIs:** org create, invite/accept, collections, multipart upload, publish, download capability, projects, chat sessions
- **Credentials:** `betaInviteToken` plaintext in mode-0600 file only; purged after gate load

## No live evidence

No qualifying `status=pass` report committed. Live POC run remains Task 17.

---

## Second review — Commit K RED tests (`071c77d`)

**Command:** `python3 -m unittest bench.markhand_web.scripts.test_phase1c_deployed_probes.Phase1cSecondReviewSliceTests -v`

**Output:**
```text
Ran 18 tests — FAILED (failures=5, errors=4)
```
New `Phase1cSecondReviewSliceTests` fail on prior code: beta-org invite/switch bootstrap, migration claim columns, public capability hashes, server-minted request IDs, exact owner-control DELETE, createUpload foreign scope, shell HUP trap without `trap -`, credential finally purge, ACL role restore, stale probe DELETE removal, REVOKE gate ordering, switch-access audit session, audit pagination UUIDs, stateful fake `execute_http_denial_suite`, denial report `challengeEcho`, UUID helper.

## Second review — Commit L GREEN implementation

**Commands & results:**
```text
python3 -m unittest bench.markhand_web.scripts.test_phase1c_deployed_probes -q       → 67/67 OK
python3 -m unittest bench.markhand_web.scripts.test_run_phase1c_gate -q              → 50/50 OK
python3 -m py_compile bench/markhand_web/scripts/phase1c_*.py run_phase1c_gate.py    → OK
python3 scripts/check-markhand-gates.py --self-test                                   → OK
```

**Implementation highlights:** beta-org bootstrap (alpha switch → beta org invite → beta accept → beta org switch); migration 0006/0007 claim/conflict SQL + list/get verification; public evidence hashes only (capabilities/tokens in 0600 creds); server UUID request IDs (no client-equality); exact owner-control mapped methods including multipart/SSE/disposable deletes; `createUpload` foreign scope; shell EXIT/HUP/INT/TERM trap with `SEED_OK` guard (no `trap -`); credential finally purge; ACL role restore; stale refresh exact 401 reuse; REVOKE last; audit switch-access `/auth/me` session + paginated cursor validation; stateful fake runs full `execute_http_denial_suite` over 53-row mapping; denial report omits unused `challengeEcho`.
