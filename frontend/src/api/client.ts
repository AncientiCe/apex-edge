/**
 * API client for ApexEdge backend. All requests go to baseUrl (e.g. http://localhost:3000).
 */

import type {
  PosRequestEnvelope,
  PosResponseEnvelope,
  PosCommand,
  CartState,
  FinalizeResult,
  ProductSearchResult,
  ProductListResponse,
  CustomerSearchResult,
  CategoryResult,
  DocumentSummary,
  DocumentResponse,
  SyncStatusResponse,
} from './types';

const VERSION = { major: 1, minor: 0, patch: 0 };

export type ApiError = {
  status: number;
  message: string;
  body?: unknown;
};

function normalizeError(status: number, body: unknown): ApiError {
  let message = `HTTP ${status}`;
  if (body && typeof body === 'object' && 'message' in body && typeof (body as { message: unknown }).message === 'string') {
    message = (body as { message: string }).message;
  }
  if (body && typeof body === 'object' && 'errors' in body && Array.isArray((body as { errors: unknown }).errors)) {
    const errs = (body as { errors: { code?: string; message?: string }[] }).errors;
    if (errs.length > 0 && errs[0].message) message = errs[0].message;
  }
  return { status, message, body };
}

export async function getHealth(baseUrl: string): Promise<{ status: string }> {
  const res = await fetch(`${baseUrl}/health`);
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  return res.json();
}

export async function getReady(baseUrl: string): Promise<{ status: string }> {
  const res = await fetch(`${baseUrl}/ready`);
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  return res.json();
}

export async function getSyncStatus(baseUrl: string): Promise<SyncStatusResponse> {
  const res = await fetch(`${baseUrl}/sync/status`);
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  return res.json();
}

/** Exact SKU lookup (returns array of 0 or 1). */
export async function searchProductsBySku(baseUrl: string, sku: string): Promise<ProductSearchResult[]> {
  const res = await fetch(`${baseUrl}/catalog/products?sku=${encodeURIComponent(sku)}`);
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  const data = await res.json();
  return Array.isArray(data) ? data : [];
}

/** List/browse products with optional search, category filter, and pagination. */
export async function listProducts(
  baseUrl: string,
  params: { q?: string; category_id?: string; page?: number; per_page?: number }
): Promise<ProductListResponse> {
  const sp = new URLSearchParams();
  if (params.q?.trim()) sp.set('q', params.q.trim());
  if (params.category_id) sp.set('category_id', params.category_id);
  if (params.page != null) sp.set('page', String(params.page));
  if (params.per_page != null) sp.set('per_page', String(params.per_page));
  const res = await fetch(`${baseUrl}/catalog/products?${sp.toString()}`);
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  const data = await res.json();
  if (data && typeof data === 'object' && 'items' in data && Array.isArray(data.items)) {
    return data as ProductListResponse;
  }
  return { items: [], total: 0, page: 1, per_page: params.per_page ?? 24 };
}

export async function listCategories(baseUrl: string): Promise<CategoryResult[]> {
  const res = await fetch(`${baseUrl}/catalog/categories`);
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  return res.json();
}

/** Search customers by name, email, code, or id (single query param q). */
export async function searchCustomers(baseUrl: string, q: string): Promise<CustomerSearchResult[]> {
  const trimmed = q.trim();
  if (!trimmed) return [];
  const res = await fetch(`${baseUrl}/customers?q=${encodeURIComponent(trimmed)}`);
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  return res.json();
}

export async function postPosCommand(
  baseUrl: string,
  envelope: PosRequestEnvelope<PosCommand>
): Promise<PosResponseEnvelope<CartState | FinalizeResult | null>> {
  const body = serializeEnvelope(envelope);
  const res = await fetch(`${baseUrl}/pos/command`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(body),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw normalizeError(res.status, data);
  return data as PosResponseEnvelope<CartState | FinalizeResult | null>;
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

/** Fetch an existing cart's current state by ID. Returns null if the cart is not found. */
export async function getCartState(baseUrl: string, cartId: string): Promise<CartState | null> {
  const res = await fetch(`${baseUrl}/pos/cart/${encodeURIComponent(cartId)}`);
  if (res.status === 404) return null;
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  return res.json();
}

export async function listOrderDocuments(baseUrl: string, orderId: string): Promise<DocumentSummary[]> {
  const res = await fetch(`${baseUrl}/orders/${orderId}/documents`);
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  return res.json();
}

export async function getDocument(baseUrl: string, id: string): Promise<DocumentResponse> {
  const res = await fetch(`${baseUrl}/documents/${id}`);
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  return res.json();
}

export async function createGiftReceipt(baseUrl: string, orderId: string): Promise<DocumentSummary> {
  const res = await fetch(`${baseUrl}/orders/${orderId}/documents/gift-receipt`, {
    method: 'POST',
  });
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  return res.json();
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
