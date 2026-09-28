import { describe, expect, it } from 'vitest';
import { lastSaleFrom } from './lastSale';
import type { CartState } from './api/types';

function cart(lines: CartState['lines']): CartState {
  return {
    cart_id: 'c',
    customer_id: null,
    state: 'paid',
    lines,
    applied_promos: [],
    applied_coupons: [],
    subtotal_cents: 0,
    discount_cents: 0,
    tax_cents: 0,
    total_cents: 0,
    tendered_cents: 0,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
  };
}

const line = {
  line_id: 'l',
  item_id: 'i',
  sku: 'EU-1',
  name: 'Espresso',
  quantity: 1,
  unit_price_cents: 1100,
  line_total_cents: 1100,
  discount_cents: 0,
  tax_cents: 100,
  modifier_option_ids: [],
  notes: null,
};

describe('lastSaleFrom', () => {
  it('refunds an inclusive line at its shelf price, not price plus tax', () => {
    const sale = lastSaleFrom('o', cart([{ ...line, tax_inclusive: true }]));
    expect(sale?.totalCents).toBe(1100);
    expect(sale?.lines[0].tax_inclusive).toBe(true);
  });

  it('adds exclusive tax on top', () => {
    const sale = lastSaleFrom('o', cart([{ ...line, unit_price_cents: 1000, line_total_cents: 1000 }]));
    expect(sale?.totalCents).toBe(1100);
    expect(sale?.lines[0].tax_inclusive).toBe(false);
  });

  it('has nothing to return for an empty cart', () => {
    expect(lastSaleFrom('o', cart([]))).toBeNull();
  });
});
