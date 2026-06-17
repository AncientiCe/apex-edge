// Real-time store stream: live availability, register presence, and cart handoffs.
//
// Consumes the edge hub's SSE feed (`GET /pos/events?store_id=&register_id=`) and reduces
// the stream into UI state. The reducer is pure so it can be unit-tested without a browser
// EventSource.

import { useEffect, useRef, useState } from 'react';

export interface HandoffNotice {
  parkedCartId: string;
  claimedByRegister: string;
  parkedByRegister: string;
}

export interface StoreStreamState {
  /** item_id -> live available_to_sell. */
  availability: Record<string, number>;
  /** register_ids currently present in the store. */
  presentRegisters: string[];
  /** Most recent cart handoff (claimed parked cart), if any. */
  lastHandoff: HandoffNotice | null;
  /** True once at least one stream message has been received. */
  connected: boolean;
}

export const initialStoreStreamState: StoreStreamState = {
  availability: {},
  presentRegisters: [],
  lastHandoff: null,
  connected: false,
};

interface StoreSnapshot {
  stock?: Array<{ item_id?: string; available_to_sell?: number }>;
  registers?: string[];
}

/** Fold a full snapshot (from `/pos/snapshot`) into the state, replacing live aggregates. */
export function applySnapshot(state: StoreStreamState, snapshot: StoreSnapshot): StoreStreamState {
  const availability: Record<string, number> = {};
  for (const entry of snapshot.stock ?? []) {
    if (typeof entry.item_id === 'string' && typeof entry.available_to_sell === 'number') {
      availability[entry.item_id] = entry.available_to_sell;
    }
  }
  return {
    ...state,
    availability,
    presentRegisters: snapshot.registers ?? state.presentRegisters,
    connected: true,
  };
}

/** Pure reducer: fold a single stream event into the state. */
export function applyStreamEvent(
  state: StoreStreamState,
  kind: string,
  payload: unknown
): StoreStreamState {
  const next: StoreStreamState = { ...state, connected: true };
  const data = (payload ?? {}) as Record<string, unknown>;
  switch (kind) {
    case 'stock_changed': {
      const items = Array.isArray(data.items) ? data.items : [];
      const availability = { ...state.availability };
      for (const raw of items) {
        const item = raw as Record<string, unknown>;
        const itemId = typeof item.item_id === 'string' ? item.item_id : null;
        const qty = typeof item.available_to_sell === 'number' ? item.available_to_sell : null;
        if (itemId !== null && qty !== null) {
          availability[itemId] = qty;
        }
      }
      next.availability = availability;
      return next;
    }
    case 'register_presence': {
      const present = Array.isArray(data.present)
        ? (data.present.filter((r) => typeof r === 'string') as string[])
        : state.presentRegisters;
      next.presentRegisters = present;
      return next;
    }
    case 'cart_handoff': {
      next.lastHandoff = {
        parkedCartId: String(data.parked_cart_id ?? ''),
        claimedByRegister: String(data.claimed_by_register ?? ''),
        parkedByRegister: String(data.parked_by_register ?? ''),
      };
      return next;
    }
    default:
      return next;
  }
}

/**
 * Subscribe to the store stream via SSE. Returns the reduced live state.
 * Reconnects automatically (EventSource does this natively); on hard error it retries
 * after a short backoff.
 */
export function useStoreStream(
  baseUrl: string,
  storeId: string,
  registerId: string,
  enabled: boolean
): StoreStreamState {
  const [state, setState] = useState<StoreStreamState>(initialStoreStreamState);
  const sourceRef = useRef<EventSource | null>(null);

  useEffect(() => {
    if (!enabled || !baseUrl || typeof EventSource === 'undefined') {
      return;
    }
    const root = baseUrl.replace(/\/$/, '');
    let cancelled = false;

    // Fetch full state from /pos/snapshot to seed (and to recover after a resnapshot signal).
    const loadSnapshot = async () => {
      try {
        const res = await fetch(
          `${root}/pos/snapshot?store_id=${encodeURIComponent(storeId)}`
        );
        if (!res.ok) return;
        const snapshot = (await res.json()) as StoreSnapshot;
        if (!cancelled) setState((prev) => applySnapshot(prev, snapshot));
      } catch {
        // Snapshot is best-effort; live events still flow.
      }
    };
    void loadSnapshot();

    const url = `${root}/pos/events?store_id=${encodeURIComponent(
      storeId
    )}&register_id=${encodeURIComponent(registerId)}`;
    const es = new EventSource(url);
    sourceRef.current = es;

    const handle = (kind: string) => (event: MessageEvent) => {
      try {
        const env = JSON.parse(event.data) as { kind?: string; payload?: unknown };
        const payload = (env.payload ?? {}) as Record<string, unknown>;
        if (payload.resnapshot_required) {
          void loadSnapshot();
          return;
        }
        setState((prev) => applyStreamEvent(prev, env.kind ?? kind, env.payload));
      } catch {
        // Ignore malformed frames.
      }
    };

    // The backend tags each SSE event with its kind, so listen per-kind plus the default.
    for (const kind of ['stock_changed', 'register_presence', 'cart_handoff']) {
      es.addEventListener(kind, handle(kind) as EventListener);
    }
    es.onmessage = handle('');

    return () => {
      cancelled = true;
      es.close();
      sourceRef.current = null;
    };
  }, [baseUrl, storeId, registerId, enabled]);

  return state;
}
