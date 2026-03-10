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
    <section className="panel connection">
      <h2>Connection</h2>
      <div className="row">
        <label>API base URL</label>
        <input
          type="url"
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          placeholder="http://localhost:3000"
          style={{ minWidth: 280 }}
        />
      </div>
      <div className="row">
        <button type="button" onClick={onCheckHealth}>
          Health
        </button>
        <span className={`status ${healthStatus === 'ok' ? 'ok' : healthStatus?.startsWith('error') ? 'err' : ''}`}>
          {healthStatus ?? '—'}
        </span>
      </div>
      <div className="row">
        <button type="button" onClick={onCheckReady}>
          Ready
        </button>
        <span className={`status ${readyStatus === 'ready' ? 'ok' : readyStatus?.startsWith('error') ? 'err' : ''}`}>
          {readyStatus ?? '—'}
        </span>
      </div>
    </section>
  );
}
