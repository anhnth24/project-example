// One tracked upload: owns its own XHR (real progress via
// `uploadTransport.ts`), and — once the upload itself succeeds with a
// `jobId` — its own job-lifecycle tracking (SSE primary, `GET /jobs/{jobId}`
// poll as fallback, per the P2-08 brief).
import { useEffect, useRef, useState } from 'react';
import { apiClient, type ApiClient } from '../../api/client';
import type { SseMessage } from '../../api/sse';
import { IconButton } from '../ui';
import { CloseIcon, SpinnerIcon } from '../icons';
import { AlertTriangle, CheckCircle2, Clock3, RotateCcw } from 'lucide-react';
import { useScopeSafeRequest } from '../../hooks/useScopeSafeRequest';
import { useScopeSafeSse } from '../../hooks/useScopeSafeSse';
import { createJobEventsSource } from './jobEventsSource';
import { describeJobPhase, isTerminalPhase, type ConversionPhase } from './jobLifecycle';
import { describeUploadFailure } from './messages';
import { startMultipartUpload } from './uploadTransport';
import type { Job, UploadOutcome, UploadProgress } from './types';

/** Fallback poll cadence for `GET /jobs/{jobId}` while a job is being tracked — a safety net behind SSE, not the primary signal. */
const POLL_INTERVAL_MS = 4000;

type RowPhase =
  | { kind: 'uploading'; progress: UploadProgress | null }
  | { kind: 'canceled' }
  | { kind: 'upload-failed'; message: string }
  | { kind: 'quarantined' }
  | { kind: 'tracking'; jobId: string };

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unitIndex]}`;
}

function conversionLabel(phase: ConversionPhase): string {
  switch (phase) {
    case 'converting':
      return 'Đang chuyển đổi sang Markdown…';
    case 'converted':
      return 'Đã chuyển đổi sang Markdown; hệ thống đang hoàn thiện lập chỉ mục để hỏi đáp.';
    case 'failed':
      return 'Chuyển đổi tài liệu thất bại. Vui lòng thử tải lại hoặc liên hệ quản trị viên.';
  }
}

export interface UploadItemRowProps {
  file: File;
  collectionId: string;
  /** Called once the upload response is known to have a `documentId` — regardless of disposition (accepted/quarantined). */
  onDocumentId: (documentId: string) => void;
  onRemove: () => void;
  /** Injectable for tests; defaults to the app-wide singleton, same convention as `LibraryPage` and `AuthProvider`. */
  client?: ApiClient;
}

export function UploadItemRow({
  file,
  collectionId,
  onDocumentId,
  onRemove,
  client = apiClient,
}: UploadItemRowProps) {
  const [phase, setPhase] = useState<RowPhase>({ kind: 'uploading', progress: null });
  // A ref, not state: only ever read from a click handler, never rendered —
  // storing it as state would force a (pointless) extra render every time
  // `startMultipartUpload` hands back its `abort`.
  const abortRef = useRef<(() => void) | null>(null);
  const [sseNudge, setSseNudge] = useState(0);
  const [pollTick, setPollTick] = useState(0);

  function applyOutcome(outcome: UploadOutcome): void {
    if (outcome.kind === 'success') {
      if (outcome.body.documentId) onDocumentId(outcome.body.documentId);
      if (outcome.body.jobId) {
        setPhase({ kind: 'tracking', jobId: outcome.body.jobId });
      } else {
        setPhase({ kind: 'quarantined' });
      }
      return;
    }
    if (outcome.kind === 'aborted') {
      setPhase({ kind: 'canceled' });
      return;
    }
    setPhase({ kind: 'upload-failed', message: describeUploadFailure(outcome) });
  }

  // Kick off the real multipart upload exactly once per (file, collectionId)
  // mount — including StrictMode's dev double-invoke, which starts a fresh
  // XHR on the second mount after the first's cleanup aborts it; no ref
  // guard here on purpose (see uploadTransport.ts's `abort()` being a safe
  // no-op post-settle, and safe pre-`send()` via `abortRequested`).
  useEffect(() => {
    let discarded = false;
    const started = startMultipartUpload({
      file,
      collectionId,
      tokenProvider: client.tokenProvider,
      onProgress: (progress) => {
        if (discarded) return;
        setPhase((current) =>
          current.kind === 'uploading' ? { kind: 'uploading', progress } : current,
        );
      },
    });
    abortRef.current = started.abort;
    started.promise.then((outcome: UploadOutcome) => {
      if (discarded) return;
      applyOutcome(outcome);
    });
    return () => {
      discarded = true;
      started.abort();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [file, collectionId]);

  const jobId = phase.kind === 'tracking' ? phase.jobId : undefined;

  // Fallback/primary source of truth: the documented `GET /jobs/{jobId}`
  // response. Read directly off this request's result (no separate
  // `phase`-synced copy) so there is exactly one place `status` lives.
  const jobRequest = useScopeSafeRequest<Job | null>(
    (signal) => {
      if (!jobId) return Promise.resolve(null);
      return client.request('get', '/jobs/{jobId}', { params: { path: { jobId } }, signal });
    },
    [jobId, sseNudge, pollTick],
  );
  const trackedJob = jobRequest.status === 'success' ? (jobRequest.data ?? null) : null;
  const conversionPhase = trackedJob ? describeJobPhase(trackedJob.status) : undefined;
  const jobTerminal = conversionPhase !== undefined && isTerminalPhase(conversionPhase);

  // Primary signal: live job events. Any message (progress or otherwise) is
  // treated as "go re-check the job now" rather than parsed for an
  // unverified payload shape — see jobLifecycle.ts's module doc for why the
  // documented `GET /jobs/{jobId}` response above is this panel's source of
  // truth instead of `SseEnvelope.data` (typed `unknown` in the contract).
  useScopeSafeSse<SseMessage>(
    () => (jobId && !jobTerminal ? createJobEventsSource(jobId, client.tokenProvider) : undefined),
    () => setSseNudge((n) => n + 1),
    [jobId, jobTerminal],
  );

  // Fallback signal: a plain poll, in case SSE never delivers (dropped
  // connection, proxy buffering, etc.) — per the P2-08 brief's own example.
  useEffect(() => {
    if (!jobId || jobTerminal) return;
    const timer = window.setInterval(() => setPollTick((t) => t + 1), POLL_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, [jobId, jobTerminal]);

  const sizeLabel = formatBytes(file.size);

  return (
    <div className="upload-item">
      <span className="upload-item-icon" aria-hidden="true">
        {phase.kind === 'uploading' && <SpinnerIcon className="spin" size={18} />}
        {phase.kind === 'tracking' && conversionPhase === 'converting' && (
          <SpinnerIcon className="spin" size={18} />
        )}
        {phase.kind === 'tracking' && conversionPhase === 'converted' && (
          <CheckCircle2 size={18} strokeWidth={2.75} />
        )}
        {phase.kind === 'tracking' && conversionPhase === 'failed' && (
          <AlertTriangle size={18} strokeWidth={2.75} />
        )}
        {phase.kind === 'tracking' && !trackedJob && <SpinnerIcon className="spin" size={18} />}
        {phase.kind === 'quarantined' && <Clock3 size={18} strokeWidth={2.75} />}
        {phase.kind === 'upload-failed' && <AlertTriangle size={18} strokeWidth={2.75} />}
        {phase.kind === 'canceled' && <RotateCcw size={18} strokeWidth={2.75} />}
      </span>

      <span className="upload-item-copy">
        <strong className="upload-item-name">{file.name}</strong>
        <span className="upload-item-meta">{sizeLabel}</span>

        {phase.kind === 'uploading' &&
          (() => {
            const total = phase.progress?.total;
            const percent =
              total !== undefined && total > 0
                ? Math.round(((phase.progress?.loaded ?? 0) / total) * 100)
                : undefined;
            return (
              <span className="upload-item-status" role="status">
                {percent !== undefined ? (
                  <>
                    <span className="upload-progress-track">
                      <span
                        className="upload-progress-value"
                        style={{ width: `${percent}%` }}
                        role="progressbar"
                        aria-valuenow={percent}
                        aria-valuemin={0}
                        aria-valuemax={100}
                        aria-label={`Đang tải lên ${file.name}`}
                      />
                    </span>
                    <span>{percent}%</span>
                  </>
                ) : (
                  <>
                    <span
                      className="upload-progress-track upload-progress-track--indeterminate"
                      role="progressbar"
                      aria-label={`Đang tải lên ${file.name}`}
                    >
                      <span className="upload-progress-value upload-progress-value--indeterminate" />
                    </span>
                    <span>Đang tải lên…</span>
                  </>
                )}
              </span>
            );
          })()}

        {phase.kind === 'tracking' && (
          <span className="upload-item-status" role="status">
            {/* Job progress has no percentage the server reports (`Job.status`
                is an enum, not a number — see jobLifecycle.ts's module doc),
                so this is a `progressbar` in the same indeterminate shape
                already used above for a non-computable upload length, never
                a fabricated aria-valuenow. Rendered only while the job is
                still running: once conversionPhase is terminal
                (converted/failed) there is nothing left to show progress
                on. */}
            {!jobTerminal && (
              <span
                className="upload-progress-track upload-progress-track--indeterminate"
                role="progressbar"
                aria-label={`Đang xử lý ${file.name}`}
              >
                <span className="upload-progress-value upload-progress-value--indeterminate" />
              </span>
            )}
            {conversionPhase
              ? conversionLabel(conversionPhase)
              : 'Đã tải lên — đang xác nhận tác vụ xử lý…'}
          </span>
        )}

        {phase.kind === 'quarantined' && (
          <span className="upload-item-status" role="status">
            Tệp đang chờ quản trị viên duyệt trước khi chuyển đổi (quarantine).
          </span>
        )}

        {phase.kind === 'canceled' && (
          <span className="upload-item-status" role="status">
            Đã hủy tải lên.
          </span>
        )}

        {phase.kind === 'upload-failed' && (
          <span className="upload-item-status" role="alert">
            {phase.message}
          </span>
        )}
      </span>

      <span className="upload-item-actions">
        {phase.kind === 'uploading' && (
          <IconButton label="Hủy tải lên" onClick={() => abortRef.current?.()}>
            <CloseIcon size={15} />
          </IconButton>
        )}
        {phase.kind !== 'uploading' && (
          <IconButton label="Bỏ khỏi danh sách" onClick={onRemove}>
            <CloseIcon size={15} />
          </IconButton>
        )}
      </span>
    </div>
  );
}
