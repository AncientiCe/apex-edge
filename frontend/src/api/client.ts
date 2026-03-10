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
  CustomerSearchResult,
  DocumentSummary,
  DocumentResponse,
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

export async function searchProducts(baseUrl: string, sku: string): Promise<ProductSearchResult[]> {
  const res = await fetch(`${baseUrl}/catalog/products?sku=${encodeURIComponent(sku)}`);
  if (!res.ok) throw normalizeError(res.status, await res.json().catch(() => ({})));
  return res.json();
}

export async function searchCustomers(baseUrl: string, code: string): Promise<CustomerSearchResult[]> {
  const res = await fetch(`${baseUrl}/customers?code=${encodeURIComponent(code)}`);
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

/** Build payload object matching backend: { action, [snake_case_variant]: payload }. */
function serializeEnvelope(envelope: PosRequestEnvelope<PosCommand>): Record<string, unknown> {
  const cmd = envelope.payload as { action: string; payload: unknown };
  const key = cmd.action;
  const payloadObj = key ? { [key]: cmd.payload } : {};
  return {
    version: envelope.version,
    idempotency_key: envelope.idempotency_key,
    store_id: envelope.store_id,
    register_id: envelope.register_id,
    payload: { action: cmd.action, ...payloadObj },
  };
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
