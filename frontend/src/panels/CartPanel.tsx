import { useState } from 'react';
import type { CartState, CartLine, CustomerSearchResult } from '../api/types';

interface Props {
  cartState: CartState | null;
  attachedCustomer: CustomerSearchResult | null;
  onGoPay: () => void;
  canPay: boolean;
  onRemoveLine: (lineId: string) => void;
  onApplyCoupon: (code: string) => void;
}

export function CartPanel({
  cartState,
  attachedCustomer,
  onGoPay,
  canPay,
  onRemoveLine,
  onApplyCoupon,
}: Props) {
  const [discountDetailsOpen, setDiscountDetailsOpen] = useState(false);
  const [couponCode, setCouponCode] = useState('');
  const lines = cartState?.lines ?? [];
  const hasDiscount =
    (cartState?.discount_cents ?? 0) > 0 ||
    (cartState?.applied_promos?.length ?? 0) > 0 ||
    (cartState?.applied_coupons?.length ?? 0) > 0;
  const couponCodeTrimmed = couponCode.trim();

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
            <button
              type="button"
              className="cart-line-remove"
              onClick={() => onRemoveLine(l.line_id)}
              aria-label={`Remove ${l.name}`}
            >
              ×
            </button>
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

      <p className="ios-section-header">Coupon</p>
      <div className="ios-card">
        <div className="coupon-input-row">
          <input
            type="text"
            value={couponCode}
            onChange={(e) => setCouponCode(e.target.value)}
            placeholder="Enter coupon code"
            className="ios-input"
            onKeyDown={(e) => {
              if (e.key === 'Enter' && couponCodeTrimmed) {
                onApplyCoupon(couponCodeTrimmed);
                setCouponCode('');
              }
            }}
          />
          <button
            type="button"
            className="btn-sm"
            onClick={() => {
              if (!couponCodeTrimmed) return;
              onApplyCoupon(couponCodeTrimmed);
              setCouponCode('');
            }}
            disabled={!couponCodeTrimmed}
          >
            Apply Coupon
          </button>
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
  const promos = cartState.applied_promos ?? [];
  const coupons = cartState.applied_coupons ?? [];
  const manualDiscounts = cartState.manual_discounts ?? [];
  const lineDiscountCents = lines.reduce((sum, line) => sum + line.discount_cents, 0);
  const couponDiscountCents = coupons.reduce((sum, coupon) => sum + coupon.discount_cents, 0);
  const manualDiscountCents = manualDiscounts.reduce((sum, discount) => sum + discount.amount_cents, 0);
  const hasSourceDetails = promos.length > 0 || coupons.length > 0 || manualDiscounts.length > 0;
  const hasAnyDiscount = cartState.discount_cents > 0 || lineDiscountCents > 0 || couponDiscountCents > 0 || manualDiscountCents > 0;
  const orderValueCents = cartState.subtotal_cents + cartState.tax_cents;
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
              <span className="cart-discount-list">${(listCents / 100).toFixed(2)}</span>
              {discountCents > 0 && (
                <>
                  <span className="cart-discount-minus">−${(discountCents / 100).toFixed(2)}</span>
                  <span className="cart-discount-eq">= ${(netCents / 100).toFixed(2)}</span>
                </>
              )}
              {discountCents === 0 && (
                <span className="cart-discount-eq">= ${(netCents / 100).toFixed(2)}</span>
              )}
            </div>
          </div>
        );
      })}
      {hasAnyDiscount && (
        <div className="cart-discount-breakdown-source">
          <p className="cart-discount-breakdown-title">Manage discounts</p>
          <div className="cart-discount-source-row">
            <span className="cart-discount-source-label">Order value</span>
            <span className="cart-discount-source-value">${(orderValueCents / 100).toFixed(2)}</span>
          </div>
          <div className="cart-discount-source-row">
            <span className="cart-discount-source-label">Total discount</span>
            <span className="cart-discount-source-value">−${(cartState.discount_cents / 100).toFixed(2)}</span>
          </div>

          <p className="cart-discount-breakdown-title">Discount totals</p>
          <div className="cart-discount-source-row">
            <span className="cart-discount-source-label">Line-level discounts</span>
            <span className="cart-discount-source-value">${(lineDiscountCents / 100).toFixed(2)}</span>
          </div>
          <div className="cart-discount-source-row">
            <span className="cart-discount-source-label">Coupon discounts</span>
            <span className="cart-discount-source-value">${(couponDiscountCents / 100).toFixed(2)}</span>
          </div>
          <div className="cart-discount-source-row">
            <span className="cart-discount-source-label">Manual discounts</span>
            <span className="cart-discount-source-value">${(manualDiscountCents / 100).toFixed(2)}</span>
          </div>
          <div className="cart-discount-source-row">
            <span className="cart-discount-source-label">Total discounts</span>
            <span className="cart-discount-source-value">${(cartState.discount_cents / 100).toFixed(2)}</span>
          </div>
          {hasSourceDetails && (
            <>
              <p className="cart-discount-breakdown-title">Automatic discounts</p>
              {promos.length > 0 && (
                <div className="cart-discount-source-row">
                  <span className="cart-discount-source-label">Promotions</span>
                  <span className="cart-discount-source-value">
                    {promos.map((promo) => (
                      <code key={promo.promo_id}>{promo.name}</code>
                    ))}
                  </span>
                </div>
              )}
              {coupons.length > 0 && (
                <>
                  <p className="cart-discount-breakdown-title">Coupons</p>
                  {coupons.map((coupon) => (
                    <div key={coupon.coupon_id} className="cart-discount-source-row">
                      <span className="cart-discount-source-label">{coupon.code}</span>
                      <span className="cart-discount-source-value">
                        −${(coupon.discount_cents / 100).toFixed(2)}
                      </span>
                    </div>
                  ))}
                </>
              )}
              {manualDiscounts.length > 0 && (
                <>
                  <p className="cart-discount-breakdown-title">Manual discounts</p>
                  {manualDiscounts.map((manual, idx) => (
                    <div key={`${manual.reason}-${idx}`} className="cart-discount-source-row">
                      <span className="cart-discount-source-label">{manual.reason}</span>
                      <span className="cart-discount-source-value">
                        −${(manual.amount_cents / 100).toFixed(2)}
                      </span>
                    </div>
                  ))}
                </>
              )}
            </>
          )}
        </div>
      )}
    </div>
  );
}
