import { describe, expect, it } from 'vitest';
import { applySnapshot, applyStreamEvent, initialStoreStreamState } from '../storeStream';

describe('applyStreamEvent', () => {
  it('updates availability from stock_changed', () => {
    const next = applyStreamEvent(initialStoreStreamState, 'stock_changed', {
      items: [
        { item_id: 'a', available_to_sell: 3 },
        { item_id: 'b', available_to_sell: 0 },
      ],
    });
    expect(next.availability).toEqual({ a: 3, b: 0 });
    expect(next.connected).toBe(true);
  });

  it('merges successive stock_changed events', () => {
    const first = applyStreamEvent(initialStoreStreamState, 'stock_changed', {
      items: [{ item_id: 'a', available_to_sell: 3 }],
    });
    const second = applyStreamEvent(first, 'stock_changed', {
      items: [{ item_id: 'b', available_to_sell: 7 }],
    });
    expect(second.availability).toEqual({ a: 3, b: 7 });
  });

  it('tracks register presence', () => {
    const next = applyStreamEvent(initialStoreStreamState, 'register_presence', {
      event: 'connected',
      present: ['r1', 'r2'],
    });
    expect(next.presentRegisters).toEqual(['r1', 'r2']);
  });

  it('records the latest cart handoff', () => {
    const next = applyStreamEvent(initialStoreStreamState, 'cart_handoff', {
      parked_cart_id: 'p1',
      claimed_by_register: 'r2',
      parked_by_register: 'r1',
    });
    expect(next.lastHandoff).toEqual({
      parkedCartId: 'p1',
      claimedByRegister: 'r2',
      parkedByRegister: 'r1',
    });
  });

  it('ignores malformed stock entries', () => {
    const next = applyStreamEvent(initialStoreStreamState, 'stock_changed', {
      items: [{ item_id: 'a' }, { available_to_sell: 5 }, 'garbage'],
    });
    expect(next.availability).toEqual({});
  });

  it('leaves state unchanged for unknown kinds (but marks connected)', () => {
    const next = applyStreamEvent(initialStoreStreamState, 'heartbeat', {});
    expect(next.availability).toEqual({});
    expect(next.presentRegisters).toEqual([]);
    expect(next.connected).toBe(true);
  });
});

describe('applySnapshot', () => {
  it('replaces availability and registers from a full snapshot', () => {
    const seeded = applyStreamEvent(initialStoreStreamState, 'stock_changed', {
      items: [{ item_id: 'stale', available_to_sell: 99 }],
    });
    const next = applySnapshot(seeded, {
      stock: [
        { item_id: 'a', available_to_sell: 4 },
        { item_id: 'b', available_to_sell: 0 },
      ],
      registers: ['r1'],
    });
    // Stale availability is replaced wholesale by the authoritative snapshot.
    expect(next.availability).toEqual({ a: 4, b: 0 });
    expect(next.presentRegisters).toEqual(['r1']);
    expect(next.connected).toBe(true);
  });

  it('tolerates an empty snapshot', () => {
    const next = applySnapshot(initialStoreStreamState, {});
    expect(next.availability).toEqual({});
    expect(next.connected).toBe(true);
  });
});
