/**
 * Public surface of the OpenAPI-driven mock. See the P2-02 report for the
 * design rationale. Typical vitest usage:
 *
 * ```ts
 * import { installMockFetch, uninstallMockFetch, resetMockState, mockControl } from '../mocks';
 *
 * beforeEach(() => { installMockFetch(); resetMockState(); });
 * afterEach(() => uninstallMockFetch());
 *
 * it('shows a 429 banner', async () => {
 *   mockControl.forceStatus('listCollections', 429, { times: 1 });
 *   // ...render the component, assert on the banner...
 * });
 * ```
 *
 * Dev-mode wiring (calling `installMockFetch()` from `main.tsx` behind an env
 * flag) is intentionally not done here — `main.tsx` belongs to app bootstrap,
 * outside `web/src/mocks/**`, which is this task's ownership boundary.
 */
export {
  installMockFetch,
  uninstallMockFetch,
  resetMockState,
  invalidateRouteCache,
} from './fetchMock';
export { mockControl } from './control';
export type { ForcedFailureKind } from './control';
export { getSpecIndex, getOperation } from './spec/openApiSpec';
export {
  registerOperation,
  getRegisteredOperations,
  DELIBERATELY_UNMOCKED_OPERATIONS,
} from './registry';
export type { MockRequestContext, MockHandlerResult, MockHandler } from './registry';
export { resetMockStore, getStore } from './fixtures';
