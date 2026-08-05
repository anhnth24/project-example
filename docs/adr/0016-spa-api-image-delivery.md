# ADR 0016: Bundle the web SPA in the API image

- Status: Proposed
- Date: 2026-08-05
- Decision key: `spa-api-image-delivery`
- Owners: web-owner, deployment-owner, security-owner
- Approver: architecture and dependency review
- Supersedes: N/A
- Related issues/PRs: UAT test-server deployment

## Context

`fileconv-server` already serves a built SPA with history fallback, immutable
asset caching, and tested security headers. The POC API image previously omitted
`web/dist`, so a remote host also needed a compatible Node/pnpm installation or
an independently managed static server. That made a same-origin, reproducible
UAT deployment depend on mutable host tooling.

The POC image supply chain is digest-pinned, and adding Node changes dependency
review and Docker cache behavior. The decision therefore belongs in the image
contract rather than an unrecorded host step.

## Decision

`deploy/Dockerfile.server` has an independent Node 22 build stage pinned by
linux/amd64 digest. It activates the repository-pinned pnpm 10.33.3, installs
only the `markhand-web` workspace dependencies, builds `web/dist`, and copies
that output into `/opt/markhand/web` in the non-root API runtime image.

The runtime sets `MARKHAND_WEB_DIST_DIR=/opt/markhand/web`; API and SPA share one
origin. The Node digest is recorded in `deploy/poc/images.lock.json`. Node,
pnpm, source maps, and package caches are absent from the runtime image.

Non-container launches keep the existing optional behavior: callers may point
`MARKHAND_WEB_DIST_DIR` at an external build, and the API still starts when no
SPA directory exists.

## Consequences

- Positive: one immutable image contains the matching API and SPA revisions.
- Positive: same-origin browser requests need no additional CORS or proxy layer.
- Positive: existing server CSP, cache-control, and SPA fallback tests remain
  the delivery contract.
- Negative: the API build now downloads a reviewed Node base and web packages.
- Negative: web source changes invalidate part of the Docker build context and
  increase cold-build time.
- Security: HSTS remains a TLS-edge responsibility. Direct HTTP is test-only
  and must not carry customer documents or production credentials.

## Alternatives considered

- Separate Nginx/CDN image: rejected for UAT because it duplicates security
  header/fallback policy and adds a proxy boundary.
- Host-built `web/dist` bind mount: rejected as the default because it is not an
  immutable, revision-coupled artifact and the test host lacks the pinned Node
  toolchain.
- Keep API-only delivery: rejected because it does not deploy the requested full
  browser flow.

## Verification

```bash
pnpm --dir web build
docker build -f deploy/Dockerfile.server -t markhand-api:poc .
deploy/scripts/poc-isolation-smoke.sh
curl --fail http://127.0.0.1:8788/
curl --fail http://127.0.0.1:8788/api/v1/health/ready
```

Verify that the runtime image has `/opt/markhand/web/index.html`, does not contain
Node/pnpm binaries, serves hashed assets with the existing cache policy, and
falls back to `index.html` only for non-API browser routes.

## Exception lifecycle

N/A.
