/**
 * Behavioral tests for API client pure functions.
 * These run in Vitest without a live backend.
 */

import { describe, it, expect } from 'vitest';
import { buildEnvelope } from '../client';

const STORE_ID = '00000000-0000-0000-0000-000000000000';
const REGISTER_ID = '11111111-1111-1111-1111-111111111111';

describe('buildEnvelope', () => {
  it('sets version to V1.0.0', () => {
    const env = buildEnvelope(STORE_ID, REGISTER_ID, {
      action: 'create_cart',
      payload: {},
    } as never);
    expect(env.version).toEqual({ major: 1, minor: 0, patch: 0 });
  });

  it('sets store_id and register_id from arguments', () => {
    const env = buildEnvelope(STORE_ID, REGISTER_ID, {
      action: 'create_cart',
      payload: {},
    } as never);
    expect(env.store_id).toBe(STORE_ID);
    expect(env.register_id).toBe(REGISTER_ID);
  });

  it('assigns a non-empty UUID as idempotency_key', () => {
    const env = buildEnvelope(STORE_ID, REGISTER_ID, {
      action: 'create_cart',
      payload: {},
    } as never);
    expect(typeof env.idempotency_key).toBe('string');
    expect(env.idempotency_key).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/
    );
  });

  it('generates a unique idempotency_key on each call', () => {
    const command = { action: 'create_cart', payload: {} } as never;
    const env1 = buildEnvelope(STORE_ID, REGISTER_ID, command);
    const env2 = buildEnvelope(STORE_ID, REGISTER_ID, command);
    expect(env1.idempotency_key).not.toBe(env2.idempotency_key);
  });

  it('passes the command through as payload', () => {
    const command = { action: 'finalize_order', payload: { cart_id: 'abc' } } as never;
    const env = buildEnvelope(STORE_ID, REGISTER_ID, command);
    expect(env.payload).toBe(command);
  });
});
