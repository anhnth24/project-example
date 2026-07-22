import type { components, paths } from './generated/contract';

type Health = components['schemas']['Health'];
const readinessPath = '/api/v1/health/ready' satisfies keyof paths;

export type ConnectionState =
  { kind: 'checking' } | { kind: 'ready'; requestId: string } | { kind: 'unavailable' };

const apiBaseUrl = import.meta.env.VITE_MARKHAND_API_BASE_URL?.replace(/\/$/, '') ?? '';

export async function fetchReadiness(signal: AbortSignal): Promise<Health> {
  const response = await fetch(`${apiBaseUrl}${readinessPath}`, {
    headers: { Accept: 'application/json' },
    signal,
  });
  if (!response.ok) {
    throw new Error(`Readiness check failed with HTTP ${response.status}`);
  }
  const payload: unknown = await response.json();
  if (!isHealth(payload)) {
    throw new Error('Readiness check returned an invalid health response');
  }
  return payload;
}

function isHealth(payload: unknown): payload is Health {
  return (
    typeof payload === 'object' &&
    payload !== null &&
    'status' in payload &&
    payload.status === 'ok' &&
    'requestId' in payload &&
    typeof payload.requestId === 'string'
  );
}
