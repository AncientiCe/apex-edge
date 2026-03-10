import { useCallback, useEffect, useState } from 'react';
import { getSyncStatus } from '../api/client';
import type { ApiError } from '../api/client';
import type { EntitySyncStatusDto, SyncStatusResponse } from '../api/types';

interface Props {
  baseUrl: string;
  disabled?: boolean;
}

function formatTime(iso: string | null): string {
  if (!iso) return '—';
  try {
    const d = new Date(iso);
    return d.toLocaleString();
  } catch {
    return iso;
  }
}

export function SyncStatusPanel({ baseUrl, disabled }: Props) {
  const [status, setStatus] = useState<SyncStatusResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const fetchStatus = useCallback(async () => {
    if (!baseUrl || disabled) return;
    setLoading(true);
    setError(null);
    try {
      const data = await getSyncStatus(baseUrl);
      setStatus(data);
    } catch (e) {
      const err = e as ApiError;
      setError(err.message);
      setStatus(null);
    } finally {
      setLoading(false);
    }
  }, [baseUrl, disabled]);

  useEffect(() => {
    void fetchStatus();
    const t = setInterval(fetchStatus, 10_000);
    return () => clearInterval(t);
  }, [fetchStatus]);

  if (disabled || !baseUrl) {
    return (
      <div className="sync-status-panel">
        <p className="ios-section-header">Sync Status</p>
        <p className="sync-status-muted">Connect to an API to see sync status.</p>
      </div>
    );
  }

  if (!status) {
    return (
      <div className="sync-status-panel">
        <p className="ios-section-header">Sync Status</p>
        {error ? (
          <>
            <p className="sync-status-error">{error}</p>
            <button type="button" className="btn-secondary" onClick={fetchStatus}>
              Retry
            </button>
          </>
        ) : (
          <p className="sync-status-muted">Loading…</p>
        )}
      </div>
    );
  }

  const data = status;
  return (
    <div className="sync-status-panel">
      <p className="ios-section-header">Sync Status</p>
      <div className="ios-card sync-status-card">
        <div className="sync-status-row">
          <span className="sync-status-label">Last sync</span>
          <span className="sync-status-value">{formatTime(data.last_sync_at)}</span>
        </div>
        <div className="sync-status-row">
          <span className="sync-status-label">State</span>
          <span className={`sync-status-badge ${data.is_syncing ? 'syncing' : 'idle'}`}>
            {data.is_syncing ? 'Syncing…' : 'Idle'}
          </span>
        </div>
      </div>
      {data.entities.length > 0 && (
        <>
          <p className="ios-section-header">Entities</p>
          <div className="sync-entities">
            {data.entities.map((ent) => (
              <EntityRow key={ent.entity} entity={ent} />
            ))}
          </div>
        </>
      )}
      {!data.last_sync_at && !data.is_syncing && data.entities.length === 0 && (
        <div className="sync-status-hint">
          <p className="ios-section-header">No sync run yet</p>
          <p className="sync-status-muted">
            Sync runs on startup and every 24h only when the app is started with{' '}
            <code>APEX_EDGE_SYNC_SOURCE_URL</code> set. Start the example sync source, then restart
            the app with the env var (e.g. <code>http://localhost:3030</code>).
          </p>
          <p className="sync-status-muted" style={{ marginTop: '0.5rem' }}>
            Example: <code>cargo run -p example-sync-source</code> on port 3030, then{' '}
            <code>APEX_EDGE_SYNC_SOURCE_URL=http://localhost:3030 cargo run -p apex-edge</code>.
          </p>
        </div>
      )}
      <div className="btn-stack" style={{ marginTop: '1rem' }}>
        <button type="button" className="btn-secondary" onClick={fetchStatus} disabled={loading}>
          Refresh
        </button>
      </div>
    </div>
  );
}

function EntityRow({ entity }: { entity: EntitySyncStatusDto }) {
  const total = entity.total ?? 0;
  const pct = entity.percent ?? (total > 0 ? (entity.current / total) * 100 : 0);
  return (
    <div className="sync-entity-row">
      <div className="sync-entity-header">
        <span className="sync-entity-name">{entity.entity}</span>
        <span className={`sync-entity-status sync-entity-status-${entity.status}`}>{entity.status}</span>
      </div>
      <div className="sync-entity-progress-row">
        <div className="sync-entity-progress-bar">
          <div
            className="sync-entity-progress-fill"
            style={{ width: `${Math.min(100, pct)}%` }}
          />
        </div>
        <span className="sync-entity-count">
          {entity.current}
          {total > 0 && ` / ${total}`}
          {entity.percent != null && ` (${entity.percent.toFixed(0)}%)`}
        </span>
      </div>
      {entity.last_synced_at && (
        <div className="sync-entity-time">Updated {formatTime(entity.last_synced_at)}</div>
      )}
    </div>
  );
}
