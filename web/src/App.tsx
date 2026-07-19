import { useCallback, useEffect, useState } from 'react';
import { fetchReadiness, type ConnectionState } from './api/health';

export function App() {
  const [connection, setConnection] = useState<ConnectionState>({ kind: 'checking' });

  const loadConnection = useCallback(async (signal?: AbortSignal) => {
    try {
      const health = await fetchReadiness(signal ?? new AbortController().signal);
      setConnection({ kind: 'ready', requestId: health.requestId });
    } catch (error) {
      if (!(error instanceof DOMException && error.name === 'AbortError')) {
        setConnection({ kind: 'unavailable' });
      }
    }
  }, []);

  useEffect(() => {
    const controller = new AbortController();
    queueMicrotask(() => {
      void loadConnection(controller.signal);
    });
    return () => controller.abort();
  }, [loadConnection]);

  const checkConnection = () => {
    setConnection({ kind: 'checking' });
    void loadConnection();
  };

  const isReady = connection.kind === 'ready';

  return (
    <div className="app-shell">
      <header className="topbar">
        <a className="brand" href="/" aria-label="Markhand home">
          <span aria-hidden="true" className="brand-mark">
            M
          </span>
          <span>Markhand</span>
        </a>
        <span className={`connection-dot ${connection.kind}`} aria-hidden="true" />
        <span className="connection-label" role="status">
          {connection.kind === 'checking' && 'Checking server'}
          {connection.kind === 'ready' && 'Server connected'}
          {connection.kind === 'unavailable' && 'Server unavailable'}
        </span>
      </header>

      <main className="welcome">
        <p className="eyebrow">Knowledge workspace</p>
        <h1>Your workspace is ready to connect.</h1>
        <p className="lede">
          Markhand will keep document conversion, indexing, and cited answers in one controlled
          workspace.
        </p>

        <section className="connection-card" aria-labelledby="connection-heading">
          <div>
            <p className="card-label">Service connection</p>
            <h2 id="connection-heading">
              {isReady ? 'Backend is available' : 'Waiting for backend'}
            </h2>
            <p className="card-copy">
              {isReady
                ? 'The document workspace can begin loading real data as API capabilities become available.'
                : 'Start the Markhand server, then check the connection again.'}
            </p>
          </div>
          <button type="button" onClick={checkConnection}>
            {connection.kind === 'checking' ? 'Checking…' : 'Check connection'}
          </button>
          {isReady && (
            <p className="request-id">
              Connected request <code>{connection.requestId}</code>
            </p>
          )}
        </section>

        <section className="next-steps" aria-labelledby="next-steps-heading">
          <h2 id="next-steps-heading">Coming next</h2>
          <ul>
            <li>Collections and secure document upload</li>
            <li>Conversion and indexing progress</li>
            <li>Search and cited answers</li>
          </ul>
        </section>
      </main>
    </div>
  );
}
