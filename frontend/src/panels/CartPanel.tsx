import type { CartState } from '../api/types';

interface Props {
  cartState: CartState | null;
  onGoPay: () => void;
  canPay: boolean;
}

export function CartPanel({
  cartState,
  onGoPay,
  canPay,
}: Props) {
  const lines = cartState?.lines ?? [];

  return (
    <section className="panel cart-panel">
      <h2>Cart</h2>
      {!cartState && <div className="status">Cart is empty.</div>}
      {cartState && (
        <div className="cart-detail">
          <ul className="cart-lines">
            {lines.map((l) => (
              <li key={l.line_id} className="cart-line">
                <div className="cart-line-main">
                  <span>{l.name}</span>
                  <span>x{l.quantity}</span>
                </div>
                <div className="cart-line-sub">
                  <span>{l.sku}</span>
                  <span>{(l.line_total_cents / 100).toFixed(2)}</span>
                </div>
              </li>
            ))}
          </ul>
          <div className="cart-totals">
            <div><span>Subtotal</span><span>{(cartState.subtotal_cents / 100).toFixed(2)}</span></div>
            <div><span>Discounts</span><span>-{(cartState.discount_cents / 100).toFixed(2)}</span></div>
            <div><span>Taxes</span><span>{(cartState.tax_cents / 100).toFixed(2)}</span></div>
            <div className="cart-total"><span>Total</span><span>{(cartState.total_cents / 100).toFixed(2)}</span></div>
          </div>
          <button type="button" className="primary" onClick={onGoPay} disabled={!canPay}>
            Pay
          </button>
        </div>
      )}
    </section>
  );
}
