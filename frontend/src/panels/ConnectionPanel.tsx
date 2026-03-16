import type { Dispatch, SetStateAction } from 'react';

interface Props {
  baseUrl: string;
  setBaseUrl: Dispatch<SetStateAction<string>>;
  healthStatus: string | null;
  readyStatus: string | null;
  storeId: string;
  setStoreId: Dispatch<SetStateAction<string>>;
  associateId: string;
  setAssociateId: Dispatch<SetStateAction<string>>;
  deviceName: string;
  setDeviceName: Dispatch<SetStateAction<string>>;
  devicePlatform: string;
  setDevicePlatform: Dispatch<SetStateAction<string>>;
  authIssuer: string;
  setAuthIssuer: Dispatch<SetStateAction<string>>;
  authAudience: string;
  setAuthAudience: Dispatch<SetStateAction<string>>;
  mockTokenSecret: string;
  setMockTokenSecret: Dispatch<SetStateAction<string>>;
  authReady: boolean;
  authBusy: boolean;
  authError: string | null;
  deviceId: string | null;
  hasDeviceSecret: boolean;
  tokenExpiresAt: string | null;
  onPairAndSignIn: () => void;
  onSignOut: () => void;
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
  storeId,
  setStoreId,
  associateId,
  setAssociateId,
  deviceName,
  setDeviceName,
  devicePlatform,
  setDevicePlatform,
  authIssuer,
  setAuthIssuer,
  authAudience,
  setAuthAudience,
  mockTokenSecret,
  setMockTokenSecret,
  authReady,
  authBusy,
  authError,
  deviceId,
  hasDeviceSecret,
  tokenExpiresAt,
  onPairAndSignIn,
  onSignOut,
  onCheckHealth,
  onCheckReady,
}: Props) {
  return (
    <div className="connection-auth-wrap connection-compact">
      <div className="connection-row">
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
      <div className="connection-row auth-fields">
        <input
          type="text"
          value={associateId}
          onChange={(e) => setAssociateId(e.target.value)}
          placeholder="associate id"
          aria-label="Associate ID"
          className="connection-url"
        />
        <input
          type="text"
          value={storeId}
          onChange={(e) => setStoreId(e.target.value)}
          placeholder="store id"
          aria-label="Store ID"
          className="connection-url"
        />
        <input
          type="text"
          value={deviceName}
          onChange={(e) => setDeviceName(e.target.value)}
          placeholder="device name"
          aria-label="Device name"
          className="connection-url"
        />
        <input
          type="text"
          value={devicePlatform}
          onChange={(e) => setDevicePlatform(e.target.value)}
          placeholder="platform"
          aria-label="Device platform"
          className="connection-url"
        />
      </div>
      <div className="connection-row auth-fields">
        <input
          type="text"
          value={authIssuer}
          onChange={(e) => setAuthIssuer(e.target.value)}
          placeholder="issuer"
          aria-label="Auth issuer"
          className="connection-url"
        />
        <input
          type="text"
          value={authAudience}
          onChange={(e) => setAuthAudience(e.target.value)}
          placeholder="audience"
          aria-label="Auth audience"
          className="connection-url"
        />
        <input
          type="password"
          value={mockTokenSecret}
          onChange={(e) => setMockTokenSecret(e.target.value)}
          placeholder="mock token secret (dev)"
          aria-label="Mock token secret"
          className="connection-url"
        />
        {!authReady ? (
          <button
            type="button"
            className="connection-status-pill"
            aria-label="Pair & Sign In"
            onClick={onPairAndSignIn}
            disabled={authBusy}
          >
            {authBusy ? 'Signing in…' : 'Pair & Sign In'}
          </button>
        ) : (
          <button
            type="button"
            className="connection-status-pill"
            aria-label="Sign out"
            onClick={onSignOut}
          >
            Sign Out
          </button>
        )}
        <span className={`auth-status ${authReady ? 'ok' : 'pending'}`}>
          {authReady ? 'Authenticated' : 'Not signed in'}
        </span>
      </div>
      {authError && <div className="auth-error">{authError}</div>}
      {authReady && (
        <div className="auth-note">
          Device: {deviceId?.slice(0, 8) ?? 'n/a'} · Credential: {hasDeviceSecret ? 'present' : 'missing'} · Access exp: {tokenExpiresAt ?? 'n/a'}
        </div>
      )}
      <div className="auth-note">Dev mode: external token is locally generated (HS256).</div>
    </div>
  );
}
