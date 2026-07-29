// P2-10. Wires `askStreamSource.ts` + the pure reducer (`state/askStream.ts`)
// through `useScopeSafeSse` (P2-06) so an org switch / logout mid-answer
// aborts the stream and discards any late message — same scope-safety every
// other live data source in this app gets, never a bespoke `useEffect`.
import { useCallback, useRef, useState } from 'react';
import type { components } from '../../api/generated/contract';
import type { TokenProvider } from '../../api/client';
import { useScopeSafeSse } from '../../hooks/useScopeSafeSse';
import {
  initialAskStreamState,
  reduceAskStreamMessage,
  type AskStreamState,
} from '../../state/askStream';
import { createAskStreamSource } from './askStreamSource';

type AskRequest = components['schemas']['AskRequest'];

export interface UseAskStreamResult {
  readonly state: AskStreamState;
  readonly isActive: boolean;
  /** Starts a brand-new ask/stream turn, replacing whatever was in flight (a fresh submit always wins — this is not a queue). */
  ask(request: AskRequest): void;
  /** Aborts the in-flight stream (if any) and returns to `idle` — used by "hủy"/unmount-equivalent UI and between demo scenarios. */
  reset(): void;
}

export function useAskStream(tokenProvider: TokenProvider): UseAskStreamResult {
  const [request, setRequest] = useState<AskRequest | undefined>(undefined);
  const [state, setState] = useState<AskStreamState>(() => initialAskStreamState());
  const sessionIdRef = useRef<string | undefined>(undefined);

  useScopeSafeSse(
    () => {
      if (!request) return undefined;
      sessionIdRef.current = undefined;
      return createAskStreamSource(request, tokenProvider, () => sessionIdRef.current);
    },
    (message) => {
      setState((prev) => {
        const next = reduceAskStreamMessage(prev, message);
        sessionIdRef.current = next.streamSessionId ?? sessionIdRef.current;
        return next;
      });
    },
    [request],
  );

  const ask = useCallback((next: AskRequest) => {
    sessionIdRef.current = undefined;
    setState(initialAskStreamState());
    setRequest(next);
  }, []);

  const reset = useCallback(() => {
    setRequest(undefined);
    setState(initialAskStreamState());
  }, []);

  return { state, isActive: request !== undefined, ask, reset };
}
