# GLM / chat provider fallback

## Detect
- Ask warnings contain provider unavailable / grounding failure.

## Contain
- Leave retrieval online; extractive answers remain available.

## Recover
1. Check `MARKHAND_GLM_*` / chat endpoint health.
2. Confirm only top-K citations are sent (never full corpus).
3. Restore provider or keep extractive mode.

## Verify
- Ask returns cited extractive or validated GLM answers; audit has no prompts.
