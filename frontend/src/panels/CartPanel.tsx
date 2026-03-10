import type { CartState, CustomerSearchResult } from '../api/types';

interface Props {
  cartState: CartState | null;
  attachedCustomer: CustomerSearchResult | null;
  onGoPay: () => void;
  canPay: boolean;
}

export function CartPanel({ cartState, attachedCustomer, onGoPay, canPay }: Props) {
  const lines = cartState?.lines ?? [];

  const customerBanner = attachedCustomer && (
    <div className="cart-customer-banner">
      <span className="cart-customer-icon">👤</span>
      <div className="cart-customer-details">
        <span className="cart-customer-label">
          {attachedCustomer.name}
          <span className="cart-customer-code"> · {attachedCustomer.code}</span>
        </span>
        {attachedCustomer.email && (
          <span className="cart-customer-email">{attachedCustomer.email}</span>
        )}
      </div>
    </div>
  );

  if (!cartState || lines.length === 0) {
    return (
      <div>
        {customerBanner}
        <p className="ios-section-header">Your Cart</p>
        <div className="ios-card">
          <div className="cart-empty">Your cart is empty.</div>
        </div>
      </div>
    );
  }

  return (
    <div>
      {customerBanner}
      <p className="ios-section-header">Items</p>
      <div className="ios-card">
        {lines.map((l) => (
          <div key={l.line_id} className="ios-row">
            <div style={{ flex: 1, minWidth: 0 }}>
              <div className="cart-line-name">{l.name}</div>
              <div className="cart-line-meta">{l.sku} · qty {l.quantity}</div>
            </div>
            <div className="ios-row-value">
              ${(l.line_total_cents / 100).toFixed(2)}
            </div>
          </div>
        ))}
      </div>

      <p className="ios-section-header">Summary</p>
      <div className="ios-card">
        <div className="ios-row">
          <span className="ios-row-title">Subtotal</span>
          <span className="ios-row-value">${(cartState.subtotal_cents / 100).toFixed(2)}</span>
        </div>
        <div className="ios-row">
          <span className="ios-row-title">Discounts</span>
          <span className="ios-row-value" style={{ color: 'var(--green)' }}>
            −${(cartState.discount_cents / 100).toFixed(2)}
          </span>
        </div>
        <div className="ios-row">
          <span className="ios-row-title">Taxes</span>
          <span className="ios-row-value">${(cartState.tax_cents / 100).toFixed(2)}</span>
        </div>
        <div className="ios-row">
          <span className="ios-row-title" style={{ fontWeight: 700 }}>Total</span>
          <span className="ios-row-value bold">${(cartState.total_cents / 100).toFixed(2)}</span>
        </div>
      </div>

      <div style={{ marginTop: '1.5rem' }}>
        <button type="button" className="btn-primary" onClick={onGoPay} disabled={!canPay}>
          Pay ${(cartState.total_cents / 100).toFixed(2)}
        </button>
      </div>
    </div>
  );
}
