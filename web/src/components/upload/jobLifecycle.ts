// Pure mapping from the documented `Job.status` enum (`api/generated/contract.ts`)
// to the document state-machine stage this panel displays
// (`uploaded → converting → converted → indexing → indexed`, `failed` —
// plans/markhand-web/phase-2-web-spa.md §P2.4). No React, unit-testable alone.
//
// [Unverified] scope limit, stated plainly rather than guessed past: the
// upload response's `jobId` is documented as a `convert` job only (`Job.jobType`
// is `"convert" | "index" | ...`, and `createUpload`'s mock/spec never returns
// a second, `index`-typed job id for a brand-new document). This file — and
// therefore this whole panel — has no server-provided way to observe the
// `indexing`/`indexed` stages: nothing in the OpenAPI contract exposes "the
// index job for this document" to a client that only holds the convert job's
// id. So `succeeded` here is reported as "converted" with copy that says
// indexing continues in the background, instead of fabricating an `indexing`
// or `indexed` state this component cannot actually confirm.
import type { Job } from './types';

export type ConversionPhase = 'converting' | 'converted' | 'failed';

const RUNNING_STATUSES: ReadonlySet<Job['status']> = new Set(['pending', 'leased', 'running']);
const FAILED_STATUSES: ReadonlySet<Job['status']> = new Set(['failed', 'cancelled', 'dead_letter']);

/** Maps a `Job`'s documented `status` to the conversion phase this panel shows. */
export function describeJobPhase(status: Job['status']): ConversionPhase {
  if (RUNNING_STATUSES.has(status)) return 'converting';
  if (FAILED_STATUSES.has(status)) return 'failed';
  return 'converted'; // only 'succeeded' remains
}

export function isTerminalPhase(phase: ConversionPhase): boolean {
  return phase === 'converted' || phase === 'failed';
}
