# Contributing to Markhand

Start with:

1. [`CLAUDE.md`](CLAUDE.md) for project priorities/native dependencies.
2. [`docs/adr/0001-web-boundaries.md`](docs/adr/0001-web-boundaries.md).
3. [`docs/conventions/delivery.md`](docs/conventions/delivery.md) for Ready/Done.
4. [`docs/runbooks/contributor-setup.md`](docs/runbooks/contributor-setup.md).

## Workflow

- One issue/outcome per logical PR; title references the roadmap issue ID.
- Run `make check-foundation` before review. Server/local-service work also runs
  `make dev-up`, `make dev-health`, then `make dev-down`.
- Public contract, tenant/security, storage topology, auth/session, migration strategy
  or native runtime changes require an ADR/security review.
- Update Markdown issue status and generated roadmap only after acceptance/evidence.
  GitHub issue/milestone state synchronizes from that status on `master`.

## Review ownership

CODEOWNERS defines required reviewers. No author self-approves security boundary or ADR
exceptions. High/critical findings need remediation or a documented owner, control,
expiry and retest date.

Never commit credentials, customer documents, model binaries or benchmark hostnames.
Report suspected secret/security exposure privately to the repository owner; do not
open a public issue containing exploit details or leaked material.
