# Phase-1B gate qualification

- Generated: `$generatedAt`
- Git commit: `$gitCommit`
- Dirty at report time: `$gitDirty`
- `targetMatch`: `$targetMatch`
- Counts: pass `$passCount`, fail `$failCount`, pending `$pendingCount`

> $caveat

## Evidence inputs

$evidenceInputs

## Gates

$gateRows

## Interpretation

- `pending` means no target-valid evidence was available for that gate.
- `measured value` is `null` unless the source evidence is target-valid.
- Synthetic, redacted, local, or sandbox evidence must not be used as a numeric
  G0-SLO/G0-CAP/DR/soak pass.
