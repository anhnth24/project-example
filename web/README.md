# Markhand Web

Browser-only React/Vite shell for the future Phase 2 SPA. It must communicate through
the generated OpenAPI contract and browser HTTP/SSE only; never import `@tauri-apps/*`
or desktop IPC.

The current shell checks the real `GET /api/v1/health/ready` endpoint. In development,
Vite proxies `/api` to `http://127.0.0.1:8787`; override this with
`MARKHAND_API_ORIGIN` when the server listens elsewhere. Production serves the SPA and
API from the same origin.
