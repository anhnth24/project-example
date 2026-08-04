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
2. **Verify the gap and owner.** Inspect current code/docs and existing issues before
   drafting. Do not create a duplicate for behavior already supported. Convert a broad
   request into one independently reviewable outcome. Do not force core/desktop/CLI work
   into a Web phase; if no owning catalog exists, obtain the owner's tracking decision.
3. **Create the authoritative record.** Write the catalog entry first using the exact
   fields and status rules in `issues-and-plans.md`. Record concrete blockers instead of
   inventing owner, approval, dependency completion, benchmark, or security evidence.
   Use `Ready` only when every Definition of Ready condition passes.
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
