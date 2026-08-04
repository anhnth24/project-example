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
2. **Gate on readiness and authoritative scope.** Verify the authoritative issue,
   dependencies, owner, consumer boundaries, and security assessment. Treat the approved
   issue scope and exclusions as authoritative. If any Ready condition is absent, or code
   reality contradicts the recorded scope, stop before implementation, retain or restore
   `Backlog`/`Blocked`, and hand the record to `issue-creator` for revision and reapproval.
   Do not infer approval, evidence, or permission to broaden the issue.
3. **Create or reuse the issue branch before file mutation.** Start from the required base
   branch and follow repository branch policy before creating the plan or changing catalog,
   roadmap, code, tests, or docs. Keep one issue per branch and logical PR.
4. **Persist the plan before production code.** Create or reuse exactly one plan under
   `plans/reports/`, using the canonical date semantics, format, status, and catalog link
   from `issues-and-plans.md`. Populate every section from repository facts and map every
   acceptance criterion to implementation location, verification, fixture/environment,
   and expected evidence. Never fabricate metadata or create a duplicate plan.
5. **Plan the technical sequence.** Inspect current contracts and invariants; identify
   files, dependencies, state/data/API transitions, negative/degraded behavior,
   cancellation/idempotency, security/migration/rollback, observability, docs, and tests.
   Escalate unclear product or destructive choices instead of guessing.
6. **Start implementation explicitly.** After the complete plan is linked and immediately
   before the first production-code change, set the plan and catalog to `In progress` and
   regenerate affected roadmap metadata. The evidence for this transition is a revalidated
   Ready issue plus the linked plan. Then implement only the mapped outcome; do not bundle
   cleanup, depend on `vendor/markitdown-rs`, change intentional pins without issue-backed
   justification, or omit lockfile updates.
7. **Apply mandatory security controls.** Trigger the required review for the areas named
   in `delivery.md`. Never self-approve exceptions or expose credentials, signed URLs,
   customer documents, model artifacts, prompts, PII, content, or secret-bearing logs.
8. **Verify proportionately and enter review.** Run focused unit, integration/contract,
   denial/security,
   migration/rollback, performance, and manual checks from the acceptance map, plus every
   applicable repository preflight in the contributor runbook. Record the actual commands,
   environment, and artifacts. Only after implementation and required pre-review evidence
   are complete, set the catalog to `Review`, keep the plan `In progress` (the plan lifecycle
   has no `Review` state), and regenerate roadmap metadata.
9. **Obtain independent review.** A reviewer other than the implementer checks the issue,
   plan, diff, negative paths, security/ADR triggers, and evidence. Remediate high/critical
   findings or record the required owner, control, expiry, and retest date.
10. **Close with evidence, not merge state.** Mark both plan and catalog `Done` only after
   independent review and every applicable Definition of Done item—including deployment,
   benchmark, acceptance, or other external gates—has evidence. Update roadmap counts,
   dependencies, final PR/commit references, commands, environment, artifacts, and
   blockers in the same closure change. Merge alone is not `Done`; use `Blocked` when a
   required external gate cannot proceed.
11. **Synchronize external state only when authorized.** Feature-branch Markdown may
   temporarily lead the remote tracker; do not claim remote consistency before the
   canonical change is merged and synchronization is verified. Then use the repository
   synchronizer to update/close the GitHub issue from the catalog. Do not create
   issues/milestones,
    deploy, release, or change remote tracking state unless the issue and user authorization
    require it. Report what is proven and what remains.
