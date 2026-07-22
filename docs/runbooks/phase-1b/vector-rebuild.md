# Vector rebuild

## Detect
- Signature mismatch, empty Qdrant generation, or intentional model cutover.

## Contain
- Keep FTS available; do not mix generations in current search.

## Recover
1. Ensure active index signature is pinned.
2. Enqueue staged backfill / index jobs for current versions.
3. Flip generation to active only after verification.

## Verify
- Retrieval golden queries pass; no shadow/retired generation leakage.
