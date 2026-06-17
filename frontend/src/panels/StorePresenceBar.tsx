import type { StoreStreamState } from '../api/storeStream';

/** Compact live indicator: connected registers + latest cart handoff. */
export function StorePresenceBar({
  state,
  registerId,
}: {
  state: StoreStreamState;
  registerId: string;
}) {
  const count = state.presentRegisters.length;
  const others = state.presentRegisters.filter((r) => r !== registerId).length;
  return (
    <div className="store-presence-bar" role="status" aria-label="Store presence">
      <span className="presence-dot" data-online={state.connected} aria-hidden />
      <span className="presence-count" data-testid="presence-count">
        {count} register{count === 1 ? '' : 's'} online
        {others > 0 ? ` (${others} other)` : ''}
      </span>
      {state.lastHandoff && (
        <span className="presence-handoff" data-testid="presence-handoff">
          Cart handed off → {state.lastHandoff.claimedByRegister.slice(0, 8)}…
        </span>
      )}
    </div>
  );
}
