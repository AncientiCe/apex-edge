export type RequestBucket = 'local' | 'non_local';

export type TrackedHttpEvent = {
  method: string;
  url: string;
  status: number | null;
  outcome: 'ok' | 'http_error' | 'network_error';
  latencyMs: number;
  bucket: RequestBucket;
  timestamp: string;
};

export type JourneyHttpSummary = {
  totalRequests: number;
  localRequests: number;
  nonLocalRequests: number;
  failedRequests: number;
  totalLatencyMs: number;
};

type TrackerState = {
  active: boolean;
  events: TrackedHttpEvent[];
  frozenSummary: JourneyHttpSummary | null;
};

const EMPTY_SUMMARY: JourneyHttpSummary = {
  totalRequests: 0,
  localRequests: 0,
  nonLocalRequests: 0,
  failedRequests: 0,
  totalLatencyMs: 0,
};

const trackerState: TrackerState = {
  active: false,
  events: [],
  frozenSummary: null,
};

function isPrivateIpv4(hostname: string): boolean {
  const parts = hostname.split('.').map((part) => Number(part));
  if (parts.length !== 4 || parts.some((part) => Number.isNaN(part) || part < 0 || part > 255)) {
    return false;
  }
  if (parts[0] === 10) return true;
  if (parts[0] === 192 && parts[1] === 168) return true;
  if (parts[0] === 172 && parts[1] >= 16 && parts[1] <= 31) return true;
  return false;
}

function isLocalHostname(hostname: string): boolean {
  const normalized = hostname.trim().toLowerCase();
  if (!normalized) return false;
  if (normalized === 'localhost') return true;
  if (normalized === '::1') return true;
  if (normalized === '0:0:0:0:0:0:0:1') return true;
  if (normalized.endsWith('.local')) return true;
  if (normalized.startsWith('127.')) return true;
  if (isPrivateIpv4(normalized)) return true;
  return false;
}

export function classifyRequestBucket(url: string): RequestBucket {
  try {
    const parsed = new URL(url);
    return isLocalHostname(parsed.hostname) ? 'local' : 'non_local';
  } catch {
    return 'non_local';
  }
}

function computeSummary(events: TrackedHttpEvent[]): JourneyHttpSummary {
  if (events.length === 0) {
    return { ...EMPTY_SUMMARY };
  }
  let localRequests = 0;
  let nonLocalRequests = 0;
  let failedRequests = 0;
  let totalLatencyMs = 0;
  for (const event of events) {
    if (event.bucket === 'local') localRequests += 1;
    if (event.bucket === 'non_local') nonLocalRequests += 1;
    if (event.outcome !== 'ok') failedRequests += 1;
    totalLatencyMs += event.latencyMs;
  }
  return {
    totalRequests: events.length,
    localRequests,
    nonLocalRequests,
    failedRequests,
    totalLatencyMs,
  };
}

export function resetJourneyTracking(): JourneyHttpSummary {
  trackerState.active = false;
  trackerState.events = [];
  trackerState.frozenSummary = null;
  return { ...EMPTY_SUMMARY };
}

export function startJourneyTracking(_context?: string): JourneyHttpSummary {
  trackerState.active = true;
  trackerState.events = [];
  trackerState.frozenSummary = null;
  return { ...EMPTY_SUMMARY };
}

export function recordHttpAttempt(
  event: Omit<TrackedHttpEvent, 'bucket' | 'timestamp'> & { bucket?: RequestBucket; timestamp?: string }
): void {
  if (!trackerState.active) return;
  const entry: TrackedHttpEvent = {
    ...event,
    bucket: event.bucket ?? classifyRequestBucket(event.url),
    timestamp: event.timestamp ?? new Date().toISOString(),
  };
  trackerState.events.push(entry);
}

export function stopJourneyTracking(_context?: string): JourneyHttpSummary {
  if (!trackerState.active) {
    return trackerState.frozenSummary ? { ...trackerState.frozenSummary } : { ...EMPTY_SUMMARY };
  }
  trackerState.active = false;
  const summary = computeSummary(trackerState.events);
  trackerState.frozenSummary = summary;
  return { ...summary };
}

export function getJourneySummary(): JourneyHttpSummary {
  if (trackerState.active) {
    return computeSummary(trackerState.events);
  }
  if (trackerState.frozenSummary) {
    return { ...trackerState.frozenSummary };
  }
  return { ...EMPTY_SUMMARY };
}
