import { useCallback, useState } from 'react';
import type { CartState, ParkedCartSummary, PosCommand, PosResponseEnvelope } from '../api/types';

/** Just enough of a completed sale to return it line by line. */
export interface LastSale {
  orderId: string;
  totalCents: number;
  lines: {
    sku: string;
    name: string;
    quantity: number;
    unit_price_cents: number;
    tax_cents: number;
  }[];
}

interface Props {
  /** Sends a command and returns the raw response; errors are surfaced by the caller. */
  send: (command: PosCommand) => Promise<PosResponseEnvelope<unknown> | null>;
  cartId: string | null;
  lastSale: LastSale | null;
  /** Owned by the app so an open shift survives switching tabs. */
  shiftId: string | null;
  onShiftChange: (shiftId: string | null) => void;
  onCartParked: () => void;
  onCartRecalled: (cart: CartState) => void;
}

type Payload = Record<string, unknown>;

function money(cents: number): string {
  const sign = cents < 0 ? '−' : '';
  return `${sign}$${(Math.abs(cents) / 100).toFixed(2)}`;
}

function toCents(input: string): number {
  const value = parseFloat(input);
  return Number.isFinite(value) && value > 0 ? Math.round(value * 100) : 0;
}

function payloadOf(res: PosResponseEnvelope<unknown> | null): Payload | null {
  return res?.success && res.payload ? (res.payload as Payload) : null;
}

/**
 * Store operations beyond a sale: till open / X report / close, parking and recalling carts,
 * and returning the last completed sale. Shows the hub doing more than cart arithmetic.
 */
export function StoreOpsPanel({
  send,
  cartId,
  lastSale,
  shiftId,
  onShiftChange,
  onCartParked,
  onCartRecalled,
}: Props) {
  const [floatInput, setFloatInput] = useState('');
  const [countedInput, setCountedInput] = useState('');
  const [expectedCents, setExpectedCents] = useState<number | null>(null);
  const [varianceCents, setVarianceCents] = useState<number | null>(null);
  const [parkNote, setParkNote] = useState('');
  const [parked, setParked] = useState<ParkedCartSummary[]>([]);
  const [returnStatus, setReturnStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const openTill = useCallback(async () => {
    const res = await send({
      action: 'open_till',
      payload: { register_id: null, associate_id: null, opening_float_cents: toCents(floatInput) },
    });
    // SHIFT_ALREADY_OPEN names the shift this register still owns (e.g. after a reload).
    const alreadyOpen = res?.errors?.some((e) => e.code === 'SHIFT_ALREADY_OPEN');
    const opened = alreadyOpen ? (res?.payload as Payload | null) : payloadOf(res);
    if (opened && typeof opened.shift_id === 'string') {
      onShiftChange(opened.shift_id);
      setExpectedCents(null);
      setVarianceCents(null);
    }
  }, [floatInput, onShiftChange, send]);

  const xReport = useCallback(async () => {
    if (!shiftId) return;
    const report = payloadOf(await send({ action: 'get_x_report', payload: { shift_id: shiftId } }));
    if (report && typeof report.expected_cents === 'number') setExpectedCents(report.expected_cents);
  }, [send, shiftId]);

  const closeTill = useCallback(async () => {
    if (!shiftId) return;
    const closed = payloadOf(
      await send({
        action: 'close_till',
        payload: { shift_id: shiftId, counted_cents: toCents(countedInput), approval_id: null },
      })
    );
    if (closed) {
      if (typeof closed.expected_cents === 'number') setExpectedCents(closed.expected_cents);
      if (typeof closed.variance_cents === 'number') setVarianceCents(closed.variance_cents);
      onShiftChange(null);
    }
  }, [countedInput, onShiftChange, send, shiftId]);

  const parkCart = useCallback(async () => {
    if (!cartId) return;
    const res = await send({
      action: 'park_cart',
      payload: { cart_id: cartId, note: parkNote.trim() || null },
    });
    if (res?.success) {
      setParkNote('');
      onCartParked();
    }
  }, [cartId, onCartParked, parkNote, send]);

  const refreshParked = useCallback(async () => {
    const res = await send({ action: 'list_parked_carts', payload: { register_id: null } });
    if (res?.success && Array.isArray(res.payload)) setParked(res.payload as ParkedCartSummary[]);
  }, [send]);

  const recall = useCallback(
    async (parkedCartId: string) => {
      const res = await send({ action: 'recall_cart', payload: { parked_cart_id: parkedCartId } });
      if (res?.success && res.payload) {
        setParked((list) => list.filter((p) => p.parked_cart_id !== parkedCartId));
        onCartRecalled(res.payload as CartState);
      }
    },
    [onCartRecalled, send]
  );

  const returnLastSale = useCallback(async () => {
    if (!lastSale) return;
    setBusy(true);
    setReturnStatus(null);
    try {
      const started = payloadOf(
        await send({
          action: 'start_return',
          payload: {
            return_id: null,
            original_order_id: lastSale.orderId,
            reason_code: 'customer_return',
            approval_id: null,
            shift_id: null,
          },
        })
      );
      if (!started || typeof started.id !== 'string') {
        setReturnStatus('Return failed at start_return');
        return;
      }
      const returnId = started.id;
      for (const line of lastSale.lines) {
        const added = await send({
          action: 'return_line_item',
          payload: { return_id: returnId, ...line, original_line_id: null },
        });
        if (!added?.success) {
          setReturnStatus('Return failed at return_line_item');
          return;
        }
      }
      const refunded = await send({
        action: 'refund_tender',
        payload: {
          return_id: returnId,
          tender_type: 'cash',
          amount_cents: lastSale.totalCents,
          external_reference: null,
        },
      });
      if (!refunded?.success) {
        setReturnStatus('Return failed at refund_tender');
        return;
      }
      const finalized = await send({ action: 'finalize_return', payload: { return_id: returnId } });
      if (!finalized?.success) {
        setReturnStatus('Return failed at finalize_return');
        return;
      }
      setReturnStatus(`Return ${returnId} finalized, refunded ${money(lastSale.totalCents)} cash`);
    } finally {
      setBusy(false);
    }
  }, [lastSale, send]);

  return (
    <div>
      <p className="ios-section-header">Till</p>
      <div className="ios-card" style={{ padding: '0.6rem 1rem', marginBottom: '0.75rem' }}>
        <div className="ios-row">
          <span className="ios-row-title">Shift</span>
          <span className="ios-row-value">{shiftId ?? 'closed'}</span>
        </div>
        {expectedCents !== null && (
          <div className="ios-row">
            <span className="ios-row-title">Expected in drawer</span>
            <span className="ios-row-value">{money(expectedCents)}</span>
          </div>
        )}
        {varianceCents !== null && (
          <div className="ios-row">
            <span className="ios-row-title">Variance</span>
            <span className="ios-row-value">{money(varianceCents)}</span>
          </div>
        )}
        <input
          type="number"
          min="0"
          step="0.01"
          aria-label="Opening float"
          placeholder="Opening float"
          className="ios-input"
          value={floatInput}
          onChange={(e) => setFloatInput(e.target.value)}
        />
        <div className="btn-stack">
          <button type="button" className="btn-secondary" onClick={openTill}>
            Open Till
          </button>
          <button type="button" className="btn-secondary" onClick={xReport} disabled={!shiftId}>
            X Report
          </button>
        </div>
        <input
          type="number"
          min="0"
          step="0.01"
          aria-label="Counted cash"
          placeholder="Counted cash"
          className="ios-input"
          value={countedInput}
          onChange={(e) => setCountedInput(e.target.value)}
        />
        <button type="button" className="btn-primary" onClick={closeTill} disabled={!shiftId}>
          Close Till
        </button>
      </div>

      <p className="ios-section-header">Parked Carts</p>
      <div className="ios-card" style={{ padding: '0.6rem 1rem', marginBottom: '0.75rem' }}>
        <input
          type="text"
          aria-label="Park note"
          placeholder="Note (optional)"
          className="ios-input"
          value={parkNote}
          onChange={(e) => setParkNote(e.target.value)}
        />
        <div className="btn-stack">
          <button type="button" className="btn-secondary" onClick={parkCart} disabled={!cartId}>
            Park Current Cart
          </button>
          <button type="button" className="btn-secondary" onClick={refreshParked}>
            Refresh Parked
          </button>
        </div>
        {parked.map((p) => {
          const label = p.note || p.parked_cart_id.slice(0, 8);
          return (
            <div className="ios-row" key={p.parked_cart_id}>
              <span className="ios-row-title">
                {label} · {p.line_count} item{p.line_count === 1 ? '' : 's'} · {money(p.total_cents)}
              </span>
              <button
                type="button"
                className="btn-secondary"
                aria-label={`Recall ${label}`}
                onClick={() => recall(p.parked_cart_id)}
              >
                Recall
              </button>
            </div>
          );
        })}
      </div>

      <p className="ios-section-header">Returns</p>
      <div className="ios-card" style={{ padding: '0.6rem 1rem' }}>
        <div className="ios-row">
          <span className="ios-row-title">Last sale</span>
          <span className="ios-row-value">
            {lastSale ? `${lastSale.orderId.slice(0, 8)}… ${money(lastSale.totalCents)}` : 'none yet'}
          </span>
        </div>
        <button
          type="button"
          className="btn-primary"
          onClick={returnLastSale}
          disabled={!lastSale || busy}
        >
          Return Last Sale
        </button>
        {returnStatus && <p className="pay-hint">{returnStatus}</p>}
      </div>
    </div>
  );
}
