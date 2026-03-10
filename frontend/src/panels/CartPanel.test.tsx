import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { CartPanel } from './CartPanel';
import type { CartState } from '../api/types';

function makeCartState(): CartState {
  return {
    cart_id: 'cart-1',
    customer_id: null,
    state: 'itemized',
    lines: [
      {
        line_id: 'line-1',
        item_id: 'item-1',
        sku: 'SKU-1',
        name: 'Coffee',
        quantity: 1,
        unit_price_cents: 350,
        line_total_cents: 350,
        discount_cents: 0,
        tax_cents: 35,
        modifier_option_ids: [],
        notes: null,
      },
    ],
    applied_promos: [],
    applied_coupons: [],
    subtotal_cents: 350,
    discount_cents: 0,
    tax_cents: 35,
    total_cents: 385,
    tendered_cents: 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

describe('CartPanel coupon apply', () => {
  it('calls onApplyCoupon with trimmed code', () => {
    const onApplyCoupon = vi.fn();
    render(
      <CartPanel
        cartState={makeCartState()}
        attachedCustomer={null}
        onGoPay={() => {}}
        canPay
        onRemoveLine={() => {}}
        onApplyCoupon={onApplyCoupon}
      />
    );

    fireEvent.change(screen.getByPlaceholderText('Enter coupon code'), {
      target: { value: '  spring10  ' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Apply Coupon' }));

    expect(onApplyCoupon).toHaveBeenCalledWith('spring10');
    expect(onApplyCoupon).toHaveBeenCalledTimes(1);
  });

  it('shows promotion identifiers and coupon sources in discount details', () => {
    const state = makeCartState();
    state.lines[0].discount_cents = 85;
    state.discount_cents = 85;
    state.total_cents = 300;
    state.applied_promos = [
      {
        promo_id: '60000000-0000-0000-0000-000000000001',
        name: 'Donna Dress 2 for 1',
        code: null,
      },
    ];
    state.applied_coupons = [
      {
        coupon_id: '80000000-0000-0000-0000-000000000001',
        code: 'SAVE20',
        discount_cents: 40,
      },
    ];
    state.manual_discounts = [{ reason: 'Manager override', amount_cents: 45, line_id: null }];

    render(
      <CartPanel
        cartState={state}
        attachedCustomer={null}
        onGoPay={() => {}}
        canPay
        onRemoveLine={() => {}}
        onApplyCoupon={() => {}}
      />
    );

    fireEvent.click(screen.getByRole('button', { name: /show discount details/i }));

    expect(screen.getByText('Donna Dress 2 for 1')).toBeDefined();
    expect(screen.getByText(/SAVE20/)).toBeDefined();
    expect(screen.getByText(/Manager override/)).toBeDefined();
    expect(screen.getByText('Discount totals')).toBeDefined();
    expect(screen.getByText('Line-level discounts')).toBeDefined();
    expect(screen.getAllByText('$0.85').length).toBeGreaterThan(0);
    expect(screen.getByText('Coupon discounts')).toBeDefined();
    expect(screen.getAllByText('$0.40').length).toBeGreaterThan(0);
    expect(screen.getAllByText('Manual discounts').length).toBeGreaterThan(0);
    expect(screen.getAllByText('$0.45').length).toBeGreaterThan(0);
    expect(screen.getByText('Total discounts')).toBeDefined();
    expect(screen.getByText('Order value')).toBeDefined();
    expect(screen.getByText('Automatic discounts')).toBeDefined();
    expect(screen.getByText('Coupons')).toBeDefined();
  });

  it('shows discount totals even when source arrays are empty', () => {
    const state = makeCartState();
    state.lines[0].discount_cents = 35;
    state.discount_cents = 35;
    state.total_cents = 350;
    state.applied_promos = [];
    state.applied_coupons = [];
    state.manual_discounts = [];

    render(
      <CartPanel
        cartState={state}
        attachedCustomer={null}
        onGoPay={() => {}}
        canPay
        onRemoveLine={() => {}}
        onApplyCoupon={() => {}}
      />
    );

    fireEvent.click(screen.getByRole('button', { name: /show discount details/i }));
    expect(screen.getByText('Discount totals')).toBeDefined();
    expect(screen.queryByText('Discount sources')).toBeNull();
  });
});
