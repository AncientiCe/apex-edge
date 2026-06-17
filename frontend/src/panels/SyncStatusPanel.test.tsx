import { render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi, beforeEach } from 'vitest';
import { SyncStatusPanel } from './SyncStatusPanel';
import type { SyncStatusResponse } from '../api/types';
import * as client from '../api/client';

function status(overrides: Partial<SyncStatusResponse>): SyncStatusResponse {
  return {
    last_sync_at: '2026-06-17T11:00:00Z',
    is_syncing: false,
    sync_staleness_seconds: 30,
    degraded: false,
    entities: [],
    ...overrides,
  };
}

describe('SyncStatusPanel degraded mode', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('shows a degraded banner when the hub is degraded', async () => {
    vi.spyOn(client, 'getSyncStatus').mockResolvedValue(
      status({ degraded: true, sync_staleness_seconds: 7200 })
    );
    render(<SyncStatusPanel baseUrl="http://localhost:3000" />);
    await waitFor(() => {
      expect(screen.getByTestId('degraded-banner')).toBeInTheDocument();
    });
    expect(screen.getByTestId('degraded-banner').textContent).toContain('Degraded mode');
    expect(screen.getByTestId('degraded-banner').textContent).toContain('2h ago');
  });

  it('does not show the banner when fresh', async () => {
    vi.spyOn(client, 'getSyncStatus').mockResolvedValue(
      status({ degraded: false, sync_staleness_seconds: 30 })
    );
    render(<SyncStatusPanel baseUrl="http://localhost:3000" />);
    await waitFor(() => {
      expect(screen.getByText('Sync Status')).toBeInTheDocument();
    });
    expect(screen.queryByTestId('degraded-banner')).not.toBeInTheDocument();
  });
});
