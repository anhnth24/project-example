---
name: issue-delivery
description: Deliver one repository issue through readiness checks, scoped implementation, independent verification, evidence, and accurate Done status.
---

# Issue delivery

Use this workflow when implementing a roadmap or GitHub issue. Treat repository delivery
documents and the issue itself as authoritative; do not infer missing approval or evidence.

1. **Validate Definition of Ready.** Confirm the issue has one independently reviewable
   outcome, explicit scope and acceptance criteria, an owner and module boundary, completed
   dependencies/external-gate evidence, data/API/tenant impact (or reasoned `N/A`), named
   test commands or fixtures, and a security-trigger assessment. If a material item is
   missing, report the exact gap and keep the issue `Blocked`/`Backlog`; do not invent it.
   Use `issue-creation` to repair an incomplete catalog issue before delivery.
2. **Create or reuse the persisted plan before editing code.** Search the issue catalog's
   `Plan file` field and `plans/reports/`; update one existing plan instead of duplicating
   it. For an open issue, use the plan creation date:
   `plans/reports/plan-YYYY-MM-DD-<lowercase-id>-<slug>.md` and `Created: YYYY-MM-DD`.
   For a historical issue already closed, use its verified GitHub close date in both the
   filename and `Issue closed: YYYY-MM-DD`. Never use today's date merely because a
   generator runs, and omit unavailable metadata instead of writing `Base commit: UNKNOWN`.
   Link the real file directly below the catalog `Status` field.
3. **Use the canonical plan structure.** Set `Status: Planned` before implementation,
   `In progress` while coding, `Blocked` with the exact blocker, and `Done` only after the
   repository Definition of Done passes. The file must contain:

   ```markdown
   # <ID> — <title>

   <Created: YYYY-MM-DD for open work, or Issue closed: YYYY-MM-DD for historical work>
   Source issue: <verified GitHub link, or state that catalog sync is pending>
   Catalog: <relative Markdown link>
   Phase plan: <relative Markdown link>
   Status: Planned

   ## Objective
   ## Context
   ## Implementation plan
   ## Files/modules
   ## Dependencies / blocks
   ## Acceptance criteria
   ## Required tests / evidence
   ## Security and migration notes
   ## Out of scope
   ## Delivery evidence
   ### Implementation PRs
   ### Recorded commit/SHA references
   ## Definition of done
   ```

   Fill every section from repository evidence. Use `N/A` with a reason where appropriate;
   use `UNKNOWN` only for a material historical fact that cannot be recovered. Do not
   fabricate links, commits, approvals, dates, tests, or delivery evidence.
4. **Build an acceptance map in the plan.** For every acceptance criterion, record the
   implementation location, automated or manual verification, fixture/environment, and
   expected evidence. Include compatibility, degraded/error behavior, migration/rollback,
   performance, docs, and deployed evidence when applicable. Preserve explicit
   out-of-scope boundaries.
5. **Plan the technical sequence.** Inspect current contracts and invariants, then order
   changes by dependency. Identify files, state/data/API transitions, failure/cancellation/
   idempotency behavior, observability without sensitive content, documentation, and
   rollback needs. Escalate unclear product or destructive choices instead of guessing.
6. **Implement one issue per branch and logical PR.** Keep changes limited to the mapped
   outcome; reference the roadmap issue ID in the PR title. Do not bundle opportunistic
   cleanup, reuse `vendor/markitdown-rs`, or change intentional dependency pins without
   issue-backed justification. Include `Cargo.lock` for dependency-manifest changes.
7. **Apply security controls.** Require security review for auth/session, org/RBAC/ACL,
   upload/converter sandbox, object storage/signed URLs, SQL/RLS/migrations, secrets/egress,
   LLM content policy, audit/logging, dependencies/native binaries, or CI permissions.
   Never self-approve security/ADR exceptions. Never commit or expose credentials, tokens,
   keys, signed URLs, customer documents, model binaries, benchmark hostnames, prompts, PII,
   or document content in logs/evidence. Report suspected exposure privately.
8. **Test proportionately.** Run focused unit, integration/contract, denial/security,
   migration/rollback, performance, and manual checks selected by the acceptance map.
   Use `make check-foundation` before review; for server/local-service work also run
   `make dev-up`, `make dev-health`, and `make dev-down`. For every Rust PR push run:
   `cargo fmt --all -- --check`,
   `cargo metadata --locked --format-version 1 --no-deps`, and
   `python3 scripts/check-dependency-policy.py`, plus relevant Rust tests. Follow
   `docs/runbooks/contributor-setup.md` for subsystem gates and test preconditions.
9. **Obtain independent verification.** Have a reviewer or independent verification pass
   check the issue, persisted plan, acceptance map, diff, negative paths, security/ADR
   triggers, and evidence. The author must not be the sole verifier. Remediate high/critical
   findings or document their owner, compensating control, expiry, and retest date.
10. **Record evidence, then update status.** Capture commands, fixture/workload,
    environment/tool versions, final commit, PR links, and artifact/report locations in the
    plan without secret- or corpus-bearing logs. Update plan status, Markdown issue status,
    blockers, and generated roadmap only after criteria and evidence pass; let repository
    synchronization manage GitHub state.
11. **Distinguish merge from Done.** A merged PR is not `Done` when deployment, benchmark,
    external gate, migration, or other post-merge evidence remains. State what is proven and
    what remains. Do not blindly create milestones/issues, deploy, release, or change remote
    tracking state; perform those actions only when the issue and explicit authorization
    require them.
