import type { components } from './generated/contract';

type Health = components['schemas']['Health'];

export type ConnectionState =
  { kind: 'checking' } | { kind: 'ready'; requestId: string } | { kind: 'unavailable' };

const apiBaseUrl = import.meta.env.VITE_MARKHAND_API_BASE_URL?.replace(/\/$/, '') ?? '';

export async function fetchReadiness(signal: AbortSignal): Promise<Health> {
  const response = await fetch(`${apiBaseUrl}/api/v1/health/ready`, {
    headers: { Accept: 'application/json' },
    signal,
  });
  if (!response.ok) {
    throw new Error(`Readiness check failed with HTTP ${response.status}`);
  }
  return (await response.json()) as Health;
}
