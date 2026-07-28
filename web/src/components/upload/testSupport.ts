// Shared test-only plumbing for `components/upload/**`'s E2E coverage. Not
// imported by any production file — same convention as
// `components/admin/testSupport.ts`.
import { getStore } from '../../mocks/fixtures';

/**
 * Marks `jobId` (a job the real `createUpload` handler already registered in
 * the store) `succeeded`, in place. Nothing in the mock ever advances a
 * job's `status` past `pending` on its own — the upload -> indexed E2E flow
 * needs exactly that transition to become observable, and there is no
 * server-driven way to produce it against the in-memory store, so this
 * exists to let a test nudge it directly instead of inventing a second,
 * parallel job store. E2E-only; no-op if `jobId` isn't a job the store
 * actually has (e.g. a stale id from a previous scenario).
 */
export function succeedJob(jobId: string): void {
  const job = getStore().jobs.get(jobId);
  if (!job) return;
  const finishedAt = new Date().toISOString();
  job.status = 'succeeded';
  job.updatedAt = finishedAt;
  job.finishedAt = finishedAt;
}
