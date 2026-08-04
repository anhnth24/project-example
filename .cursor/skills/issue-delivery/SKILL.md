---
name: issue-delivery
description: Deliver one repository issue through readiness checks, scoped implementation, independent verification, evidence, and accurate Done status.
---

# Issue delivery

Use this workflow to deliver one Ready issue. The user prompt only needs the issue ID and
special constraints; this skill owns plan creation, implementation discipline, review,
evidence, and status transitions.

1. **Load the mandatory contract.** Read and follow:
   - `docs/conventions/issues-and-plans.md` for plan naming, metadata, sections, acceptance
     mapping, evidence, lifecycle, and catalog linking;
   - `docs/conventions/delivery.md` for Definition of Ready/Done and security triggers;
   - `CONTRIBUTING.md` and `docs/runbooks/contributor-setup.md` for workflow/quality gates;
   - `CLAUDE.md` and relevant ADRs for architecture constraints.
   These requirements apply even when the prompt does not repeat them.
2. **Gate on readiness.** Verify the authoritative issue and dependencies. If any material
   Ready condition is absent, state the exact gap, retain `Backlog`/`Blocked`, and use
   `issue-creator` to repair the record. Do not infer approval or evidence.
3. **Persist the plan before code.** Create or reuse exactly one plan under
   `plans/reports/`, using the canonical date semantics, format, status, and catalog link
   from `issues-and-plans.md`. Populate every section from repository facts and map every
   acceptance criterion to implementation location, verification, fixture/environment,
   and expected evidence. Never fabricate metadata or create a duplicate plan.
4. **Plan the technical sequence.** Inspect current contracts and invariants; identify
   files, dependencies, state/data/API transitions, negative/degraded behavior,
   cancellation/idempotency, security/migration/rollback, observability, docs, and tests.
   Escalate unclear product or destructive choices instead of guessing.
5. **Implement one issue per branch and logical PR.** Keep changes limited to the mapped
   outcome. Do not bundle opportunistic cleanup, depend on `vendor/markitdown-rs`, change
   intentional pins without issue-backed justification, or omit lockfile updates.
6. **Apply mandatory security controls.** Trigger the required review for the areas named
   in `delivery.md`. Never self-approve exceptions or expose credentials, signed URLs,
   customer documents, model artifacts, prompts, PII, content, or secret-bearing logs.
7. **Verify proportionately.** Run focused unit, integration/contract, denial/security,
   migration/rollback, performance, and manual checks from the acceptance map, plus every
   applicable repository preflight in the contributor runbook.
8. **Obtain independent review.** A reviewer other than the implementer checks the issue,
   plan, diff, negative paths, security/ADR triggers, and evidence. Remediate high/critical
   findings or record the required owner, control, expiry, and retest date.
9. **Record evidence and transition status.** Keep plan, catalog, roadmap, PR, commit,
   commands, environment, artifacts, blockers, and GitHub representation consistent.
   Mark `Done` only when the documented Definition of Done passes; merge alone is not Done.
10. **Mutate external state only when authorized.** Do not create issues/milestones,
    deploy, release, or change remote tracking state unless the issue and user authorization
    require it. Report what is proven and what remains.
