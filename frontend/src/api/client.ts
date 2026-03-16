/**
 * API client for ApexEdge backend. All requests go to baseUrl (e.g. http://localhost:3000).
 */

import type {
  AuthCreatePairingCodeRequest,
  AuthCreatePairingCodeResponse,
  AuthDevicePairRequest,
  AuthDevicePairResponse,
  AuthSessionExchangeRequest,
  AuthSessionExchangeResponse,
  AuthSessionRefreshRequest,
  AuthSessionRevokeResponse,
  CartState,
  CategoryResult,
  ContractVersion,
  CustomerSearchResult,
  DocumentResponse,
  DocumentSummary,
  FinalizeResult,
  PosCommand,
  PosRequestEnvelope,
  PosResponseEnvelope,
  ProductListResponse,
  ProductSearchResult,
  SyncStatusResponse,
} from './types';

const VERSION: ContractVersion = { major: 1, minor: 0, patch: 0 };

export type ApiError = {
  status: number;
  message: string;
  body?: unknown;
};

export type AuthTokens = {
  accessToken: string;
  refreshToken: string;
  expiresAt: string;
  refreshExpiresAt: string;
};

type AuthTransport = {
  getAccessToken: () => string | null;
  getRefreshToken: () => string | null;
  onTokens: (tokens: AuthTokens) => void;
  onAuthFailure: (reason: string) => void;
};

let authTransport: AuthTransport | null = null;

export function configureAuthTransport(transport: AuthTransport | null) {
  authTransport = transport;
}

function normalizeError(status: number, body: unknown): ApiError {
  let message = `HTTP ${status}`;
  if (
    body &&
    typeof body === 'object' &&
    'message' in body &&
    typeof (body as { message: unknown }).message === 'string'
  ) {
    message = (body as { message: string }).message;
  }
  if (
    body &&
    typeof body === 'object' &&
    'errors' in body &&
    Array.isArray((body as { errors: unknown }).errors)
  ) {
    const errs = (body as { errors: { message?: string }[] }).errors;
    if (errs.length > 0 && errs[0].message) message = errs[0].message;
  }
  return { status, message, body };
}

function mergeHeaders(
  headers: HeadersInit | undefined,
  accessToken: string
): Record<string, string> {
  const out: Record<string, string> = {};
  if (headers) {
    if (Array.isArray(headers)) {
      headers.forEach(([k, v]) => {
        out[k] = v;
      });
    } else if (headers instanceof Headers) {
      headers.forEach((v, k) => {
        out[k] = v;
      });
    } else {
      Object.assign(out, headers);
    }
  }
  out.Authorization = `Bearer ${accessToken}`;
  return out;
}

async function fetchJson<T>(
  url: string,
  init?: RequestInit
): Promise<T> {
  const res = await fetch(url, init);
  const body = await res.json().catch(() => ({}));
  if (!res.ok) throw normalizeError(res.status, body);
  return body as T;
}

async function fetchWithAuth<T>(
  baseUrl: string,
  path: string,
  init?: RequestInit
): Promise<T> {
  if (!authTransport) {
    throw normalizeError(401, { message: 'Auth transport not configured' });
  }
  const accessToken = authTransport.getAccessToken();
  if (!accessToken) {
    authTransport.onAuthFailure('missing_access_token');
    throw normalizeError(401, { message: 'Missing access token' });
  }

  const first = await fetch(`${baseUrl}${path}`, {
    ...init,
    headers: mergeHeaders(init?.headers, accessToken),
  });
  const firstBody = await first.json().catch(() => ({}));
  if (first.ok) return firstBody as T;
  if (first.status !== 401) throw normalizeError(first.status, firstBody);

  const refreshToken = authTransport.getRefreshToken();
  if (!refreshToken) {
    authTransport.onAuthFailure('missing_refresh_token');
    throw normalizeError(401, firstBody);
  }
  try {
    const refreshed = await refreshSession(baseUrl, refreshToken);
    authTransport.onTokens({
      accessToken: refreshed.access_token,
      refreshToken: refreshed.refresh_token,
      expiresAt: refreshed.expires_at,
      refreshExpiresAt: refreshed.refresh_expires_at,
    });
  } catch {
    authTransport.onAuthFailure('refresh_failed');
    throw normalizeError(401, firstBody);
  }

  const nextAccessToken = authTransport.getAccessToken();
  if (!nextAccessToken) {
    authTransport.onAuthFailure('refresh_missing_access');
    throw normalizeError(401, firstBody);
  }
  const retry = await fetch(`${baseUrl}${path}`, {
    ...init,
    headers: mergeHeaders(init?.headers, nextAccessToken),
  });
  const retryBody = await retry.json().catch(() => ({}));
  if (!retry.ok) {
    if (retry.status === 401) authTransport.onAuthFailure('retry_unauthorized');
    throw normalizeError(retry.status, retryBody);
  }
  return retryBody as T;
}

/** Base64url encode UTF-8 string. */
function base64UrlEncodeText(input: string): string {
  const bytes = new TextEncoder().encode(input);
  let binary = '';
  bytes.forEach((b) => {
    binary += String.fromCharCode(b);
  });
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
}

/** Base64url encode bytes. */
function base64UrlEncodeBytes(bytes: Uint8Array): string {
  let binary = '';
  bytes.forEach((b) => {
    binary += String.fromCharCode(b);
  });
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/g, '');
}

export async function generateMockExternalToken(params: {
  associateId: string;
  issuer: string;
  audience: string;
  storeId: string;
  secret: string;
  expiresInSeconds?: number;
}): Promise<string> {
  const now = Math.floor(Date.now() / 1000);
  const exp = now + (params.expiresInSeconds ?? 600);
  const header = { alg: 'HS256', typ: 'JWT' };
  const payload = {
    sub: params.associateId,
    iss: params.issuer,
    aud: params.audience,
    exp,
    iat: now,
    store_id: params.storeId,
  };
  const encodedHeader = base64UrlEncodeText(JSON.stringify(header));
  const encodedPayload = base64UrlEncodeText(JSON.stringify(payload));
  const signingInput = `${encodedHeader}.${encodedPayload}`;
  const key = await crypto.subtle.importKey(
    'raw',
    new TextEncoder().encode(params.secret),
    { name: 'HMAC', hash: 'SHA-256' },
    false,
    ['sign']
  );
  const signature = await crypto.subtle.sign(
    'HMAC',
    key,
    new TextEncoder().encode(signingInput)
  );
  const encodedSig = base64UrlEncodeBytes(new Uint8Array(signature));
  return `${signingInput}.${encodedSig}`;
}

export async function getHealth(baseUrl: string): Promise<{ status: string }> {
  return fetchJson(`${baseUrl}/health`);
}

export async function getReady(baseUrl: string): Promise<{ status: string }> {
  return fetchJson(`${baseUrl}/ready`);
}

export async function getSyncStatus(baseUrl: string): Promise<SyncStatusResponse> {
  return fetchWithAuth(baseUrl, '/sync/status');
}

export async function searchProductsBySku(
  baseUrl: string,
  sku: string
): Promise<ProductSearchResult[]> {
  return fetchWithAuth(baseUrl, `/catalog/products?sku=${encodeURIComponent(sku)}`);
}

export async function getProductById(
  baseUrl: string,
  id: string
): Promise<ProductSearchResult | null> {
  try {
    return await fetchWithAuth(baseUrl, `/catalog/products/${encodeURIComponent(id)}`);
  } catch (e) {
    const err = e as ApiError;
    if (err.status === 404) return null;
    throw err;
  }
}

export async function listProducts(
  baseUrl: string,
  params: { q?: string; category_id?: string; page?: number; per_page?: number }
): Promise<ProductListResponse> {
  const sp = new URLSearchParams();
  if (params.q?.trim()) sp.set('q', params.q.trim());
  if (params.category_id) sp.set('category_id', params.category_id);
  if (params.page != null) sp.set('page', String(params.page));
  if (params.per_page != null) sp.set('per_page', String(params.per_page));
  const query = sp.toString();
  const data = await fetchWithAuth<unknown>(
    baseUrl,
    `/catalog/products${query ? `?${query}` : ''}`
  );
  if (data && typeof data === 'object' && 'items' in data && Array.isArray((data as ProductListResponse).items)) {
    return data as ProductListResponse;
  }
  return { items: [], total: 0, page: 1, per_page: params.per_page ?? 24 };
}

export async function listCategories(baseUrl: string): Promise<CategoryResult[]> {
  return fetchWithAuth(baseUrl, '/catalog/categories');
}

export async function searchCustomers(
  baseUrl: string,
  q: string
): Promise<CustomerSearchResult[]> {
  const trimmed = q.trim();
  if (!trimmed) return [];
  return fetchWithAuth(baseUrl, `/customers?q=${encodeURIComponent(trimmed)}`);
}

/** Build payload object matching backend internal-tagged enum: { action, ...payloadFields }. */
function serializeEnvelope(envelope: PosRequestEnvelope<PosCommand>): Record<string, unknown> {
  const cmd = envelope.payload as { action: string; payload: unknown };
  const payloadObj =
    cmd.payload && typeof cmd.payload === 'object'
      ? (cmd.payload as Record<string, unknown>)
      : {};
  return {
    version: envelope.version,
    idempotency_key: envelope.idempotency_key,
    store_id: envelope.store_id,
    register_id: envelope.register_id,
    payload: { action: cmd.action, ...payloadObj },
  };
}

export async function postPosCommand(
  baseUrl: string,
  envelope: PosRequestEnvelope<PosCommand>
): Promise<PosResponseEnvelope<CartState | FinalizeResult | null>> {
  const body = serializeEnvelope(envelope);
  return fetchWithAuth(baseUrl, '/pos/command', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
}

export async function getCartState(baseUrl: string, cartId: string): Promise<CartState | null> {
  try {
    return await fetchWithAuth(baseUrl, `/pos/cart/${encodeURIComponent(cartId)}`);
  } catch (e) {
    const err = e as ApiError;
    if (err.status === 404) return null;
    throw err;
  }
}

export async function listOrderDocuments(
  baseUrl: string,
  orderId: string
): Promise<DocumentSummary[]> {
  return fetchWithAuth(baseUrl, `/orders/${orderId}/documents`);
}

export async function getDocument(baseUrl: string, id: string): Promise<DocumentResponse> {
  return fetchWithAuth(baseUrl, `/documents/${id}`);
}

export async function createGiftReceipt(
  baseUrl: string,
  orderId: string
): Promise<DocumentSummary> {
  return fetchWithAuth(baseUrl, `/orders/${orderId}/documents/gift-receipt`, {
    method: 'POST',
  });
}

export async function createPairingCode(
  baseUrl: string,
  req: AuthCreatePairingCodeRequest
): Promise<AuthCreatePairingCodeResponse> {
  return fetchJson(`${baseUrl}/auth/pairing-codes`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(req),
  });
}

export async function pairDevice(
  baseUrl: string,
  req: AuthDevicePairRequest
): Promise<AuthDevicePairResponse> {
  return fetchJson(`${baseUrl}/auth/devices/pair`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(req),
  });
}

export async function exchangeSession(
  baseUrl: string,
  req: AuthSessionExchangeRequest
): Promise<AuthSessionExchangeResponse> {
  return fetchJson(`${baseUrl}/auth/sessions/exchange`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(req),
  });
}

export async function refreshSession(
  baseUrl: string,
  refreshToken: string
): Promise<AuthSessionExchangeResponse> {
  const payload: AuthSessionRefreshRequest = { refresh_token: refreshToken };
  return fetchJson(`${baseUrl}/auth/sessions/refresh`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
}

export async function revokeSession(
  baseUrl: string,
  accessToken: string
): Promise<AuthSessionRevokeResponse> {
  return fetchJson(`${baseUrl}/auth/sessions/revoke`, {
    method: 'POST',
    headers: { Authorization: `Bearer ${accessToken}` },
  });
}

export function buildEnvelope(
  storeId: string,
  registerId: string,
  command: PosCommand
): PosRequestEnvelope<PosCommand> {
  return {
    version: VERSION,
    idempotency_key: crypto.randomUUID(),
    store_id: storeId,
    register_id: registerId,
    payload: command,
  };
}
