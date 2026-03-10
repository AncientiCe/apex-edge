import type { Dispatch, SetStateAction } from 'react';

interface Props {
  baseUrl: string;
  setBaseUrl: Dispatch<SetStateAction<string>>;
  healthStatus: string | null;
  readyStatus: string | null;
  onCheckHealth: () => void;
  onCheckReady: () => void;
}

function dotClass(status: string | null, okValue: string): string {
  if (!status) return 'pending';
  if (status === okValue) return 'ok';
  if (status.startsWith('error')) return 'err';
  return 'pending';
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
    <div className="connection-row connection-compact">
      <input
        type="url"
        value={baseUrl}
        onChange={(e) => setBaseUrl(e.target.value)}
        placeholder="API URL"
        aria-label="API base URL"
        className="connection-url"
      />
      <button
        type="button"
        className="connection-status-pill"
        onClick={onCheckHealth}
        aria-label="Check health"
      >
        <span className={`dot ${dotClass(healthStatus, 'ok')}`} />
        Health{healthStatus && healthStatus !== 'ok' && healthStatus !== '—' ? `: ${healthStatus}` : ''}
      </button>
      <button
        type="button"
        className="connection-status-pill"
        onClick={onCheckReady}
        aria-label="Check ready"
      >
        <span className={`dot ${dotClass(readyStatus, 'ready')}`} />
        Ready{readyStatus && readyStatus !== 'ready' && readyStatus !== '—' ? `: ${readyStatus}` : ''}
      </button>
    </div>
  );
}
