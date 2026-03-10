import type { Dispatch, SetStateAction } from 'react';

interface Props {
  baseUrl: string;
  setBaseUrl: Dispatch<SetStateAction<string>>;
  healthStatus: string | null;
  readyStatus: string | null;
  onCheckHealth: () => void;
  onCheckReady: () => void;
}

export function ConnectionPanel({
  baseUrl,
  setBaseUrl,
  healthStatus,
  readyStatus,
  onCheckHealth,
  onCheckReady,
}: Props) {
  return (
    <section className="panel connection connection-compact">
      <h2>Connection</h2>
      <div className="connection-row">
        <input
          type="url"
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          placeholder="API URL"
          aria-label="API base URL"
          className="connection-url"
        />
        <button type="button" onClick={onCheckHealth} aria-label="Check health">
          Health
        </button>
        <span className={`status ${healthStatus === 'ok' ? 'ok' : healthStatus?.startsWith('error') ? 'err' : ''}`}>
          {healthStatus ?? '—'}
        </span>
        <button type="button" onClick={onCheckReady} aria-label="Check ready">
          Ready
        </button>
        <span className={`status ${readyStatus === 'ready' ? 'ok' : readyStatus?.startsWith('error') ? 'err' : ''}`}>
          {readyStatus ?? '—'}
        </span>
      </div>
    </section>
  );
}
