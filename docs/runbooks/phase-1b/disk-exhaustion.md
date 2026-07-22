# Disk exhaustion

## Detect
- Host / volume alerts; worker temp write failures.

## Contain
- Stop convert/index workers.
- Preserve quarantine and trusted prefixes.

## Recover
1. Expand volume or purge expired temp/workspace dirs only.
2. Never delete trusted objects without reconcile dry-run.
3. Resume workers with resource limits intact.

## Verify
- Convert/index succeed; no unbounded temp growth under soak.
