# ApexEdge failover & recovery runbook

**Applies to:** ApexEdge v0.6.0+.

ApexEdge is a local store hub. It runs on a store-LAN box (bare metal, VM, or
container — whatever the retailer already runs). The POS/MPOS talk to it on the
LAN. This runbook covers three scenarios, ordered by frequency:

1. **Box recovery** — the hub process crashed or the host rebooted.
2. **Disaster recovery** — the hub box is destroyed and you need to bring a fresh
   one online from backups.
3. **On-LAN failover** (optional, for high-volume stores that run two hubs).

## Health signals

| Signal | Source | Meaning |
| --- | --- | --- |
| `GET /health` → 200 | hub | process alive |
| `GET /audit/verify` → `{ok: true}` | hub | audit chain intact |
| `apex_edge_role{role="primary"} == 1` | `/metrics` | this hub is primary |
| `apex_edge_role{role="standby"} == 1` | `/metrics` | this hub is standby |
| `X-ApexEdge-Role` response header | every HTTP response | role visible to POS |
| `503 + Retry-After: 30` on POST | standby rejecting writes | guard triggered |

## 1. Box recovery (most common)

The hub process crashed or the host rebooted. SQLite + WAL is crash-safe and the
outbox is transactional, so recovery is "just start it again."

1. Start the hub binary (systemd / docker / whatever the store uses).
2. `curl -sf http://hub.lan:3000/health` must return 200.
3. `curl -sf http://hub.lan:3000/audit/verify` must return `{"ok": true}`.
   - If `ok: false`, the DB was tampered with or corrupted. Go to §2.
4. The outbox dispatcher resumes automatically. Pending submissions retry with
   exponential backoff; HQ rejects duplicates via deterministic `submission_id`.

The crash-recovery proptest (`cargo test -p apex-edge-storage --test
crash_recovery_proptest`) is the contract: any operation sequence that was
interrupted mid-flight replays cleanly on the next start.

## 2. Disaster recovery (box destroyed)

The hub hardware is gone, the disk is unreadable, or the site is restoring from a
backup. This is where Litestream earns its keep: continuous WAL shipment to an
object store means the new box can be up to date within seconds of the last
shipped frame.

### 2a. Pre-reqs (configure **once per store**)

Each store should ship WAL to an object store. Litestream is the reference
implementation; any WAL-shipping tool works. Minimum example Litestream config
(run as a sidecar process alongside the hub):

```yaml
# /etc/litestream.yml on the hub box
dbs:
  - path: /data/apex_edge.db
    replicas:
      - url: s3://apex-edge-backups/${APEX_EDGE_STORE_ID}/apex_edge.db
        retention: 168h
        snapshot-interval: 1h
        sync-interval: 1s
```

Then `litestream replicate -config /etc/litestream.yml` runs as a daemon.

### 2b. Restore procedure

1. Provision a new hub box (same Dockerfile / binary / OS image as before).
2. Before starting the hub, restore the DB:
   ```bash
   litestream restore -o /data/apex_edge.db \
     s3://apex-edge-backups/${APEX_EDGE_STORE_ID}/apex_edge.db
   ```
3. Copy the same `audit.key` the old box used, or point `APEX_EDGE_AUDIT_KEY_PATH`
   at your secrets store. If the key is lost, the chain is verifiable up to the
   key rotation point and a new key simply chains forward from the current tip.
4. Start the hub binary.
5. Verify: `/health` = 200, `/audit/verify` = `{ok: true}`.
6. Re-attach POS/MPOS devices — they auto-reconnect to `/pos/stream`.
7. Resume Litestream replication from the new box.

Typical RTO on a small store: minutes. RPO ≤ `sync-interval` (1s by default).

## 3. On-LAN failover (optional, high-volume stores only)

Some retailers want sub-minute failover without a restore step. For that, run a
second hub on the same LAN in standby mode and flip roles when the primary dies.

### 3a. Standby setup

- Stand up a second hub box on the store LAN.
- Set `APEX_EDGE_STANDBY=1` on it.
- Point its Litestream sidecar at the **same** object-store path as the primary,
  so it stays continuously up to date.
- The standby `standby_guard` middleware rejects any POST/PUT/PATCH/DELETE with
  `503 Service Unavailable + Retry-After: 30 + X-ApexEdge-Role: standby`. Reads
  (cart browsing, catalog lookups, `/audit/verify`) keep working.
- POS clients should detect the `X-ApexEdge-Role: standby` header and show a
  "hub in maintenance" banner.

### 3b. Planned failover (maintenance window)

1. Broadcast a `sync_progress` `maintenance_start` message on `/pos/stream` so
   tills show a banner.
2. Drain the primary's outbox dispatcher (stop the process, or let it quiesce).
3. Wait for `apex_edge_wal_replication_lag_seconds` ≤ 1s.
4. Flip roles:
   - Primary: set `APEX_EDGE_STANDBY=1`, restart.
   - Standby: unset `APEX_EDGE_STANDBY`, restart.
5. Verify on the new primary: `curl -sf .../health && curl -sf .../audit/verify`.
6. Update whatever routes POS traffic (DNS A-record, systemd override, load
   balancer, or static config on the tills) to point at the new primary.
7. Resume the outbox dispatcher. POS devices reconnect to `/pos/stream` and drop
   the maintenance banner.

### 3c. Unplanned failover (primary crashed)

1. **Confirm primary is dead** (not just slow). A split-brain promotion corrupts
   the outbox because both hubs would re-submit deterministic envelopes under
   different shift/order ids.
2. Snapshot the object store: `litestream restore -o /tmp/primary-snapshot.db …`
   so you have a known-good point-in-time before promotion.
3. Promote standby: unset `APEX_EDGE_STANDBY`, restart.
4. `curl /audit/verify` — if `ok: false`, you lost data in flight. File an
   incident; `first_bad_id` tells you where the chain broke.
5. Bring the former primary back as the **new** standby once repaired: wipe its
   local DB, set `APEX_EDGE_STANDBY=1`, start. Litestream reseeds it.

## POS client contract

When the POS receives `503 + X-ApexEdge-Role: standby` on a write:

1. Show a "hub in maintenance" banner.
2. Stop retrying write commands for at least `Retry-After` seconds.
3. Reads keep working — basket browsing, price lookups continue.

On `/pos/stream` reconnect, the POS should call `/audit/verify` so it detects a
role flip immediately and updates the banner.

## Drills

Run in staging monthly:

```bash
make smoke-loop &        # continuous synthetic traffic
# crash the hub (or the primary, in HA mode)
kill -9 $(pgrep apex-edge)
# start the new primary per §1 or §3
```

**Success criteria:**

- 100% `apex_edge_synthetic_journey_total{step="end_to_end",outcome="success"}` during the drill
- clean `/audit/verify` after recovery
- ≤ 60s of write unavailability (HA mode) or ≤ documented RTO (DR mode)
