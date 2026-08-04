---
name: issue-creator
description: Create or revise a Markhand roadmap issue in the authoritative catalog, validate Definition of Ready, and safely synchronize the canonical format to GitHub.
---

# Issue creator

Use this workflow when decomposing a phase into issues or adding one roadmap issue. The
Markdown phase catalog is authoritative; GitHub issues are synchronized representations.
Do not create a remote issue, milestone, or label unless the user explicitly requests it.

1. **Locate the roadmap authority.** Read `plans/markhand-web/README.md`, the phase plan,
   and `plans/markhand-web/backlog/<phase>/issues/README.md`. Check existing IDs, phase
   issue count, dependency graph, default status, milestone, and similarly scoped issues.
   Never reuse an ID or create a second issue for the same independently reviewable outcome.
2. **Choose one outcome and status.** The title must be `ID — concise outcome`, where the
   ID follows that phase's existing convention. Use only `Backlog`, `Blocked`, `Ready`,
   `In progress`, `Review`, or `Done`. New work is normally `Backlog` or `Blocked`; use
   `Ready` only after every Definition of Ready check below passes. Never create a new
   issue directly as `Done`.
3. **Write the catalog entry first.** Use these canonical field names so
   `scripts/sync-github-issues.py` can generate the GitHub body:

   ```markdown
   ## <ID> — <concise outcome>

   - **Status:** <Backlog | Blocked — exact blocker | Ready>
   - **Objective:** <one observable, independently reviewable outcome>
   - **Implementation plan:** <ordered technical approach and failure/degraded behavior>
   - **Files/modules:** <owner plus module boundaries and likely files>
   - **Dependencies/blocks:** <issue IDs/external gates and what evidence unblocks them>
   - **Acceptance criteria:** <observable pass/fail behaviors, including negative paths>
   - **Required tests/evidence:** <named commands, fixtures, environments, and artifacts>
   - **Security/migration:** <data/API/tenant impact; review trigger; migration/rollback,
     or reasoned N/A>
   - **Out of scope:** <explicit exclusions that prevent scope growth>
   ```

   Keep acceptance criteria separate from implementation tasks. Do not invent approvals,
   owners, dependency completion, benchmark results, or security evidence. Record unknown
   material facts as blockers instead.
4. **Apply Definition of Ready.** Before using `Ready`, confirm: one outcome; explicit
   scope and acceptance criteria; owner and module boundary; dependencies completed with
   evidence; data/API/tenant impact stated; named test/evidence commands or fixtures;
   security trigger assessed; and out-of-scope boundaries recorded. Otherwise retain
   `Backlog`/`Blocked` and state the exact gap.
5. **Maintain roadmap consistency.** Add the issue to the phase dependency graph and
   summary where applicable. When adding a new ID, update the phase issue count and the
   `Issue-level backlog (... issues)` total in `plans/markhand-web/README.md`. Do not add a
   `Plan file` placeholder: `issue-delivery` creates and links the real plan before code.
6. **Validate before synchronization.** Run:

   ```bash
   python3 scripts/build-roadmap.py
   python3 scripts/build-roadmap.py --check
   python3 scripts/sync-github-issues.py --dry-run
   ```

   Review the generated title, phase, status, and issue count. Fix catalog/parser errors;
   do not bypass them.
7. **Synchronize only when authorized.** With explicit permission, use the repository
   synchronizer rather than manually duplicating the body:

   ```bash
   python3 scripts/sync-github-issues.py --create
   # Use --update when an existing catalog issue changed.
   ```

   Confirm the GitHub title is `ID — title`, and that milestone, phase/status labels,
   source paths, and rendered sections match the catalog. If remote write access is
   unavailable, deliver the catalog change and report that synchronization remains.
8. **Hand off to delivery.** Report the issue ID, status, catalog/phase links, blockers,
   and validation evidence. Invoke `issue-delivery` only after the issue is `Ready`; that
   workflow creates the detailed plan, branch, implementation, review, and Done evidence.
