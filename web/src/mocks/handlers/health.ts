import { registerOperation } from '../registry';
import { nextRequestId } from '../ids';
import { specText } from '../spec/specSource';

function health() {
  return { status: 200, body: { status: 'ok' as const, requestId: nextRequestId() } };
}

registerOperation('healthLive', () => health());
registerOperation('healthReady', () => health());
registerOperation('healthStart', () => health());

// Served verbatim, from the exact same source file the rest of the mock is
// derived from — this endpoint can't drift from the spec because it *is* the
// spec's own text.
registerOperation('openapiYaml', () => ({
  status: 200,
  rawBody: { text: specText, contentType: 'application/yaml' },
}));
