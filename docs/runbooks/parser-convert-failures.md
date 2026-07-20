# Parser and conversion failures

Use this when uploads are accepted but conversion jobs fail, time out, or produce
unsafe output. Conversion runs in the isolated worker container and shells out to
the project-owned `fileconv` CLI.

## Detection

- `MarkhandJobDeadLetterOrFailedGrowth` with `job_type="convert"`.
- `MarkhandJobThroughputBelowPeakGate` for `job_type="convert"`.
- Logs from `worker-convert` with `worker job failed`.
- User-visible job status from `GET /api/v1/jobs/{jobId}`.

## Triage

```bash
docker compose -f deploy/compose.poc.yml logs --since=30m worker-convert
curl -fsS http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/health/ready
curl -fsS -H "Authorization: Bearer $TOKEN" \
  http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/jobs/$JOB_ID
```

Classify the failure:

- Unsupported or spoofed format rejected by upload validation.
- Converter timeout, memory, process or file-size sandbox limit.
- Missing native runtime in the worker image, such as PDFium, Tesseract or OCR data.
- Parser bug or degraded extraction quality.
- Object-store download/staging failure.

For a de-identified reproduction file, reproduce outside the worker first:

```bash
./target/release/fileconv one path/to/repro-file > /tmp/repro.md
```

Do not copy customer files into issue trackers or logs.

## Contain

1. If the converter is failing all files, pause only conversion:

   ```bash
   docker compose -f deploy/compose.poc.yml stop worker-convert
   ```

2. Keep search, ask, preview and download online for already indexed documents.
3. If one format is causing repeated failures, temporarily block that format at the
   intake policy or ask operators to stop accepting it until the parser is fixed.
4. Do not increase sandbox limits for untrusted files unless security signs off.

## Recover

- Restore missing native runtime in the worker image or host mount, then rebuild:

  ```bash
  docker compose -f deploy/compose.poc.yml build worker-convert
  docker compose -f deploy/compose.poc.yml up -d worker-convert
  ```

- If the source file was malformed, ask the owner to upload a corrected file.
- If Markdown was produced but indexing failed later, use the reindex endpoint after
  fixing the index path:

  ```bash
  curl -fsS -X POST -H "Authorization: Bearer $TOKEN" \
    http://127.0.0.1:${MARKHAND_POC_SERVER_PORT:-8787}/api/v1/documents/$DOCUMENT_ID:reindex
  ```

- For parser code fixes, rerun format-specific CLI tests and the upload vertical
  slice before re-enabling the worker.

## Verify

- `markhand_jobs_processed_total{job_type="convert",outcome="success"}` increases.
- No new `markhand_jobs_processed_total{job_type="convert",outcome=~"failed|dead_letter"}` growth over 30 minutes.
- The user's document reaches the expected version/index state.
- Preview/download still require authorization and do not expose quarantine objects.
