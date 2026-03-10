import { useState } from 'react';
import type { CartState, CartLine, CustomerSearchResult } from '../api/types';

interface Props {
  cartState: CartState | null;
  attachedCustomer: CustomerSearchResult | null;
  onGoPay: () => void;
  canPay: boolean;
}

export function CartPanel({ cartState, attachedCustomer, onGoPay, canPay }: Props) {
  const [discountDetailsOpen, setDiscountDetailsOpen] = useState(false);
  const lines = cartState?.lines ?? [];
  const hasDiscount =
    (cartState?.discount_cents ?? 0) > 0 ||
    (cartState?.applied_promos?.length ?? 0) > 0 ||
    (cartState?.applied_coupons?.length ?? 0) > 0;

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
        {hasDiscount && (
          <>
            <button
              type="button"
              className="cart-discount-details-toggle"
              onClick={() => setDiscountDetailsOpen((o) => !o)}
              aria-expanded={discountDetailsOpen}
            >
              <span className="cart-discount-details-toggle-text">
                {discountDetailsOpen ? 'Hide' : 'Show'} discount details
              </span>
              <span className="cart-discount-details-toggle-icon">{discountDetailsOpen ? '▼' : '▶'}</span>
            </button>
            {discountDetailsOpen && (
              <div className="cart-discount-details">
                <DiscountBreakdown lines={lines} cartState={cartState} />
              </div>
            )}
          </>
        )}
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

function DiscountBreakdown({
  lines,
  cartState,
}: {
  lines: CartLine[];
  cartState: CartState;
}) {
  const promoCount = cartState.applied_promos?.length ?? 0;
  const coupons = cartState.applied_coupons ?? [];
  return (
    <div className="cart-discount-breakdown">
      <p className="cart-discount-breakdown-title">Per item</p>
      {lines.map((l) => {
        const listCents = l.line_total_cents;
        const discountCents = l.discount_cents;
        const netCents = listCents - discountCents;
        return (
          <div key={l.line_id} className="cart-discount-line">
            <div className="cart-discount-line-name">
              {l.name} × {l.quantity}
            </div>
            <div className="cart-discount-line-amounts">
              <span>${(listCents / 100).toFixed(2)}</span>
              {discountCents > 0 && (
                <>
                  <span className="cart-discount-minus">−${(discountCents / 100).toFixed(2)}</span>
                  <span className="cart-discount-eq">= ${(netCents / 100).toFixed(2)}</span>
                </>
              )}
            </div>
          </div>
        );
      })}
      {(promoCount > 0 || coupons.length > 0) && (
        <p className="cart-discount-breakdown-source">
          {promoCount > 0 && (
            <span>{promoCount} automatic promotion{promoCount !== 1 ? 's' : ''} applied.</span>
          )}
          {coupons.length > 0 && (
            <span>
              {' '}
              Coupon{coupons.length !== 1 ? 's' : ''}:{' '}
              {coupons.map((c) => `${c.code} (−$${(c.discount_cents / 100).toFixed(2)})`).join(', ')}
            </span>
          )}
        </p>
      )}
    </div>
  );
}
