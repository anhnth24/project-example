# Converter outbreak

## Detect
- Spike in convert failures / timeouts.
- Worker sandbox preflight failures.

## Contain
- Scale convert workers to zero.
- Leave quarantine objects; do not promote.

## Recover
1. Run `fileconv-worker --sandbox-preflight`.
2. Confirm native deps (pdfium/tesseract) in worker image.
3. Re-enable one worker; soak a synthetic corpus sample.
4. Resume remaining workers.

## Verify
- Convert success rate recovers; no host FS / egress violations in logs.
