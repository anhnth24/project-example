// Shared type aliases for the library/collection/document UI (P2-07,
// plans/markhand-web/phase-2-web-spa.md §P2.4). Every shape here is derived
// straight from `api/generated/contract.ts` — nothing re-declares a schema
// that file already exports. `Document` is aliased to `LibraryDocument` so
// importers never accidentally shadow the DOM's global `Document` type.
import type { components } from '../../api/generated/contract';

export type Collection = components['schemas']['Collection'];
export type LibraryDocument = components['schemas']['Document'];
export type DocumentState = LibraryDocument['state'];
export type PageInfo = components['schemas']['PageInfo'];
/** P2-18 — org -> project -> collection -> document grouping. */
export type Project = components['schemas']['Project'];
