import { useState } from 'react';
import type { CartState, FinalizeResult } from '../api/types';
import type { PosCommand } from '../api/types';

const TENDER_ID = '00000000-0000-0000-0000-000000000001';

interface Props {
  cartId: string | null;
  cartState: CartState | null;
  finalizeResult: FinalizeResult | null;
  onPosCommand: (cmd: PosCommand) => void;
}

export function CheckoutPanel({ cartId, cartState, finalizeResult, onPosCommand }: Props) {
  const [amountCents, setAmountCents] = useState('');

  const handleSetTendering = () => {
    if (!cartId) return;
    onPosCommand({ action: 'set_tendering', payload: { cart_id: cartId } });
  };

  const handleAddPayment = () => {
    if (!cartId) return;
    const cents = Math.round(parseFloat(amountCents) * 100) || 0;
    if (cents <= 0) return;
    onPosCommand({
      action: 'add_payment',
      payload: {
        cart_id: cartId,
        tender_id: TENDER_ID,
        amount_cents: cents,
        external_reference: null,
      },
    });
  };

  const handleFinalize = () => {
    if (!cartId) return;
    onPosCommand({ action: 'finalize_order', payload: { cart_id: cartId } });
  };

  const canTender = cartId && cartState && ['open', 'itemized', 'discounted'].includes(cartState.state);
  const canPay = cartId && cartState?.state === 'tendering';
  const canFinalize = cartId && cartState?.state === 'paid';

  return (
    <section className="panel checkout">
      <h2>Checkout</h2>
      <div className="row">
        <button type="button" onClick={handleSetTendering} disabled={!canTender}>
          Set tendering
        </button>
      </div>
      <div className="row">
        <input
          type="number"
          step="0.01"
          min="0"
          placeholder="Amount"
          value={amountCents}
          onChange={(e) => setAmountCents(e.target.value)}
          style={{ width: 100 }}
        />
        <button type="button" onClick={handleAddPayment} disabled={!canPay}>
          Add payment
        </button>
      </div>
      <div className="row">
        <button type="button" onClick={handleFinalize} disabled={!canFinalize}>
          Finalize order
        </button>
      </div>
      {finalizeResult && (
        <div className="pre" style={{ marginTop: '0.5rem', fontSize: '0.875rem' }}>
          <div>Order ID: {finalizeResult.order_id}</div>
          <div>Total: {(finalizeResult.total_cents / 100).toFixed(2)}</div>
          <div>Print jobs: {finalizeResult.print_job_ids?.length ?? 0}</div>
        </div>
      )}
    </section>
  );
}
