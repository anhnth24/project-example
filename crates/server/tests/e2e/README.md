# P1B-O04 vertical-slice/security e2e suite

Target: `e2e_suite` (`crates/server/tests/e2e_suite.rs`).

Live run:

```bash
cargo build -p fileconv
set -a; source /tmp/live-test.env; set +a
CC=gcc CXX=g++ cargo test -p fileconv-server --test e2e_suite -- --test-threads=1 --nocapture
```

Scenarios:

- `live_full_vertical_slice_multi_format`: upload txt/csv/html/markdown fixtures, create documents, run real convert and index workers, then verify search, ask, preview, download, and citation resolution.
- `live_authorization_e2e_unauthorized_gets_no_text`: no-ACL, no-`qa.query`, and cross-tenant users cannot obtain indexed text through search/ask/citation/preview/download/document/job routes.
- `live_lifecycle_delete_purge_and_revocation`: delete tombstones and purge through workers; search/ask/preview/citation no longer expose text; ACL revocation is honored on the next request.
- `live_adversarial_malicious_input_rejected_or_contained`: malformed multipart, spoofed content, cross-tenant object keys, traversal keys, and citation pin mismatches fail closed without partial ingest or content leak.
- `live_fault_injection_worker_kill_retry_consistency`: a convert worker loses its lease after staging, a second worker completes, and indexing finishes with one consistent chunk/vector set.

The target self-skips without PostgreSQL, MinIO, Qdrant, sandbox support, or a built `fileconv` binary. The simple-format vertical slice is expected to pass in the live test environment after `cargo build -p fileconv`.
