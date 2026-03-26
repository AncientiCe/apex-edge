/**
 * Behavioral tests for API client pure functions.
 * These run in Vitest without a live backend.
 */

import { beforeEach, describe, it, expect, vi, afterEach } from 'vitest';
import {
  buildEnvelope,
  configureAuthTransport,
  getHealth,
  getJourneySummary,
  listCategories,
  postPosCommand,
  resetJourneyTracking,
  refreshSession,
  startJourneyTracking,
  stopJourneyTracking,
} from '../client';

const STORE_ID = '00000000-0000-0000-0000-000000000000';
const REGISTER_ID = '11111111-1111-1111-1111-111111111111';

describe('buildEnvelope', () => {
  it('sets version to V1.0.0', () => {
    const env = buildEnvelope(STORE_ID, REGISTER_ID, {
      action: 'create_cart',
      payload: {},
    } as never);
    expect(env.version).toEqual({ major: 1, minor: 0, patch: 0 });
  });

  it('sets store_id and register_id from arguments', () => {
    const env = buildEnvelope(STORE_ID, REGISTER_ID, {
      action: 'create_cart',
      payload: {},
    } as never);
    expect(env.store_id).toBe(STORE_ID);
    expect(env.register_id).toBe(REGISTER_ID);
  });

  it('assigns a non-empty UUID as idempotency_key', () => {
    const env = buildEnvelope(STORE_ID, REGISTER_ID, {
      action: 'create_cart',
      payload: {},
    } as never);
    expect(typeof env.idempotency_key).toBe('string');
    expect(env.idempotency_key).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
    );
  });

  it('generates a unique idempotency_key on each call', () => {
    const command = { action: 'create_cart', payload: {} } as never;
    const env1 = buildEnvelope(STORE_ID, REGISTER_ID, command);
    const env2 = buildEnvelope(STORE_ID, REGISTER_ID, command);
    expect(env1.idempotency_key).not.toBe(env2.idempotency_key);
  });

  it('passes the command through as payload', () => {
    const command = { action: 'finalize_order', payload: { cart_id: 'abc' } } as never;
    const env = buildEnvelope(STORE_ID, REGISTER_ID, command);
    expect(env.payload).toBe(command);
  });
});

describe('auth transport', () => {
  const originalFetch = global.fetch;
  const baseUrl = 'http://localhost:3000';

  beforeEach(() => {
    vi.restoreAllMocks();
    configureAuthTransport(null);
    resetJourneyTracking();
  });

  afterEach(() => {
    global.fetch = originalFetch;
  });

  it('attaches bearer token to protected requests', async () => {
    const fetchMock = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => [],
    })) as unknown as typeof fetch;
    global.fetch = fetchMock;
    configureAuthTransport({
      getAccessToken: () => 'access-123',
      getRefreshToken: () => null,
      onTokens: () => {},
      onAuthFailure: () => {},
    });
    await listCategories(baseUrl);
    const init = fetchMock.mock.calls[0][1] as RequestInit;
    expect((init.headers as Record<string, string>).Authorization).toBe(
      'Bearer access-123'
    );
  });

  it('refreshes once on 401 and retries protected request', async () => {
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce({
        ok: false,
        status: 401,
        json: async () => ({ message: 'unauthorized' }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          access_token: 'new-access',
          refresh_token: 'new-refresh',
          expires_at: new Date(Date.now() + 60_000).toISOString(),
          refresh_expires_at: new Date(Date.now() + 3_600_000).toISOString(),
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => [],
      }) as unknown as typeof fetch;
    global.fetch = fetchMock;

    let access = 'old-access';
    let refresh = 'old-refresh';
    configureAuthTransport({
      getAccessToken: () => access,
      getRefreshToken: () => refresh,
      onTokens: (tokens) => {
        access = tokens.accessToken;
        refresh = tokens.refreshToken;
      },
      onAuthFailure: () => {},
    });

    await listCategories(baseUrl);
    expect(fetchMock).toHaveBeenCalledTimes(3);
    const retriedInit = fetchMock.mock.calls[2][1] as RequestInit;
    expect((retriedInit.headers as Record<string, string>).Authorization).toBe(
      'Bearer new-access'
    );
  });

  it('fails hard when refresh fails', async () => {
    const onAuthFailure = vi.fn();
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce({
        ok: false,
        status: 401,
        json: async () => ({ message: 'unauthorized' }),
      })
      .mockResolvedValueOnce({
        ok: false,
        status: 401,
        json: async () => ({ message: 'refresh failed' }),
      }) as unknown as typeof fetch;
    global.fetch = fetchMock;

    configureAuthTransport({
      getAccessToken: () => 'old-access',
      getRefreshToken: () => 'old-refresh',
      onTokens: () => {},
      onAuthFailure,
    });

    await expect(listCategories(baseUrl)).rejects.toMatchObject({ status: 401 });
    expect(onAuthFailure).toHaveBeenCalledTimes(1);
  });
});

describe('journey http tracker', () => {
  const originalFetch = global.fetch;

  beforeEach(() => {
    vi.restoreAllMocks();
    configureAuthTransport(null);
    resetJourneyTracking();
  });

  afterEach(() => {
    global.fetch = originalFetch;
  });

  it('counts wire-level attempts including auth retry and refresh', async () => {
    const baseUrl = 'http://localhost:3000';
    const fetchMock = vi
      .fn()
      .mockResolvedValueOnce({
        ok: false,
        status: 401,
        json: async () => ({ message: 'unauthorized' }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => ({
          access_token: 'new-access',
          refresh_token: 'new-refresh',
          expires_at: new Date(Date.now() + 60_000).toISOString(),
          refresh_expires_at: new Date(Date.now() + 3_600_000).toISOString(),
        }),
      })
      .mockResolvedValueOnce({
        ok: true,
        status: 200,
        json: async () => [],
      }) as unknown as typeof fetch;
    global.fetch = fetchMock;

    let access = 'old-access';
    let refresh = 'old-refresh';
    configureAuthTransport({
      getAccessToken: () => access,
      getRefreshToken: () => refresh,
      onTokens: (tokens) => {
        access = tokens.accessToken;
        refresh = tokens.refreshToken;
      },
      onAuthFailure: () => {},
    });

    startJourneyTracking('test');
    await listCategories(baseUrl);
    const stopped = stopJourneyTracking('done');
    const summary = getJourneySummary();

    expect(fetchMock).toHaveBeenCalledTimes(3);
    expect(stopped.totalRequests).toBe(3);
    expect(summary.totalRequests).toBe(3);
    expect(summary.localRequests).toBe(3);
    expect(summary.nonLocalRequests).toBe(0);
    expect(summary.failedRequests).toBe(1);
    expect(summary.totalLatencyMs).toBeGreaterThanOrEqual(0);
  });

  it('classifies non-local requests and tracks errors', async () => {
    const baseUrl = 'https://api.example.com';
    const fetchMock = vi.fn(async () => ({
      ok: false,
      status: 404,
      json: async () => ({ message: 'missing' }),
    })) as unknown as typeof fetch;
    global.fetch = fetchMock;

    startJourneyTracking('test');
    await expect(getHealth(baseUrl)).rejects.toMatchObject({ status: 404 });
    const summary = stopJourneyTracking('done');

    expect(summary.totalRequests).toBe(1);
    expect(summary.localRequests).toBe(0);
    expect(summary.nonLocalRequests).toBe(1);
    expect(summary.failedRequests).toBe(1);
  });
});

describe('auth endpoints', () => {
  const originalFetch = global.fetch;
  const baseUrl = 'http://localhost:3000';

  afterEach(() => {
    global.fetch = originalFetch;
  });

  it('refreshSession posts refresh token payload', async () => {
    const fetchMock = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => ({
        access_token: 'a',
        refresh_token: 'r',
        expires_at: new Date().toISOString(),
        refresh_expires_at: new Date().toISOString(),
      }),
    })) as unknown as typeof fetch;
    global.fetch = fetchMock;
    await refreshSession(baseUrl, 'refresh-token');
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe(`${baseUrl}/auth/sessions/refresh`);
    expect((init as RequestInit).method).toBe('POST');
  });

  it('postPosCommand uses auth transport for protected route', async () => {
    const fetchMock = vi.fn(async () => ({
      ok: true,
      status: 200,
      json: async () => ({
        version: { major: 1, minor: 0, patch: 0 },
        success: true,
        idempotency_key: 'x',
        payload: null,
        errors: [],
      }),
    })) as unknown as typeof fetch;
    global.fetch = fetchMock;
    configureAuthTransport({
      getAccessToken: () => 'access-xyz',
      getRefreshToken: () => null,
      onTokens: () => {},
      onAuthFailure: () => {},
    });
    await postPosCommand(baseUrl, buildEnvelope(STORE_ID, REGISTER_ID, {
      action: 'create_cart',
      payload: {},
    } as never));
    const init = fetchMock.mock.calls[0][1] as RequestInit;
    expect((init.headers as Record<string, string>).Authorization).toBe('Bearer access-xyz');
  });
});
