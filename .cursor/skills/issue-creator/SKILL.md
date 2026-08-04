---
name: issue-creator
description: Create or revise a Markhand roadmap issue in the authoritative catalog, validate Definition of Ready, and safely synchronize the canonical format to GitHub.
---

# Issue creator

Use this workflow when decomposing a phase or creating/revising one issue. A draft prompt
only needs the outcome and special product constraints. Drafting never mutates GitHub;
the canonical approval phrase defined below authorizes automatic synchronization.

1. **Load the mandatory contract.** Read and follow:
   - `docs/conventions/issues-and-plans.md` for canonical issue format, authority, status,
     roadmap consistency, synchronization, and prompt contract;
   - `docs/conventions/delivery.md` for Definition of Ready/Done and security triggers;
   - the owning roadmap/catalog and phase plan;
   - `CLAUDE.md` for repository architecture constraints.
   These documents supply mandatory requirements even when the prompt omits them.
   Keep the output at issue granularity: outcome, ownership, boundaries, dependencies,
   observable acceptance/evidence, security trigger, and exclusions. The canonical
   `Implementation plan` field contains only high-level technical direction plus
   failure/degraded behavior. Do not create a plan file, task-by-task sequence, or detailed
   acceptance mapping here; `issue-delivery` owns those after the issue is `Ready`.
2. **Verify the gap, owner, tracking location, and consumer impact.** Inspect current
   code/docs and existing issues before drafting. Do not create a duplicate for behavior
   already supported. Convert a broad request into one independently reviewable outcome.
   Before writing the draft, resolve these two required decisions when the prompt does not:
   - **Tracking:** whenever the owner, tracking location, catalog, or milestone is unknown
     or ambiguous—including when no catalog exists or multiple locations look plausible—
     present the viable choices and ask the user/owner to decide. Do not choose a
     convenient active phase or reopen a completed phase by inference. Do not force
     core/desktop/CLI work into a Web phase.
   - **Consumer scope:** for a shared core/library/API, enumerate every direct consumer
     and any gate that runs before the shared code. Ask whether the outcome is core-only
     or end-to-end for named consumers when that choice changes files, tests,
     dependencies, security review, or acceptance evidence. Do not assume every consumer
     automatically inherits the core behavior. If another boundary/owner is required,
     propose a separate dependent issue instead of silently broadening or hiding the gap.
   Record the answers in `Objective`, `Files/modules`, `Dependencies/blocks`,
   `Acceptance criteria`, `Required tests/evidence`, and `Out of scope`. For example, a
   TXT UTF-16 core request must distinguish direct CLI/desktop/MCP calls from a server
   upload gate that may reject the bytes before core.
3. **Create or reuse the issue branch before file mutation.** Start from the required base
   branch and follow repository branch policy before changing the catalog or roadmap. Keep
   one issue per branch and logical PR. Then write the catalog entry first using the exact
   fields and status rules in `issues-and-plans.md`. Record concrete blockers instead of
   inventing owner, approval, dependency completion, benchmark, or security evidence. Use
   `Ready` only when every Definition of Ready condition passes.
4. **Keep the roadmap consistent.** Update dependency graph, phase summary, phase count,
   total count, and generated roadmap as required by the owning catalog. Do not create a
   placeholder plan link; `issue-delivery` creates and links the real file.
5. **Validate the rendered issue.** For Markhand Web run the roadmap build/check and
   `python3 scripts/sync-github-issues.py --dry-run` commands required by
   `issues-and-plans.md`. Run relevant repository static checks and fix parser/count drift.
6. **Treat draft approval as the sync trigger.** When the user says `Tôi duyệt draft.`,
   revalidate the current draft against Definition of Ready. If it passes, change the
   catalog status to `Ready`, update and validate roadmap metadata, then automatically
   create the missing GitHub issue or update the existing one with the repository
   synchronizer. The user does not need to request GitHub sync separately. Verify title,
   milestone, labels, body, and source links, then return the issue URL.
7. **Fail closed.** Approval does not waive readiness. If a required fact/evidence is
   missing, do not mark `Ready` or create the GitHub issue; report the exact gap. Do not
   manually diverge the GitHub body from the catalog. If authentication or authorization
   fails, preserve the catalog as authority and report the remaining sync action.
8. **Hand off concisely.** Return the issue ID, title, status, authority links, blockers,
   and validation evidence. `issue-delivery` may start only when the issue is `Ready`.
