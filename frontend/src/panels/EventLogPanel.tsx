import type { LogEntry } from '../App';

interface Props {
  entries: LogEntry[];
}

export function EventLogPanel({ entries }: Props) {
  return (
    <section className="panel log">
      <h2>Event log</h2>
      <div className="event-log">
        {entries.length === 0 && <div className="status">No events yet.</div>}
        {entries.map((e, i) => (
          <div key={i} className={`event-entry ${e.kind}`}>
            [{e.ts}] {e.text}
          </div>
        ))}
      </div>
    </section>
  );
}
