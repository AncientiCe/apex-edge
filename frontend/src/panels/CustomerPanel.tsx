import { useCallback, useState } from 'react';
import type { CustomerSearchResult } from '../api/types';

interface Props {
  onSearch: (q: string) => void;
  customers: CustomerSearchResult[];
  onSelectCustomer: (customerId: string) => void;
  selectedCustomerId: string | null;
  disabled: boolean;
}

export function CustomerPanel({
  onSearch,
  customers,
  onSelectCustomer,
  selectedCustomerId,
  disabled,
}: Props) {
  const [q, setQ] = useState('');

  const handleSearch = useCallback(() => {
    onSearch(q);
  }, [q, onSearch]);

  return (
    <section className="panel customer-panel">
      <h2>Customer</h2>
      <div className="row">
        <input
          type="text"
          placeholder="Name, email, code, or ID"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
          aria-label="Search customer"
          className="customer-search"
        />
        <button type="button" onClick={handleSearch} disabled={disabled}>
          Search
        </button>
      </div>
      <ul className="customer-list">
        {customers.length === 0 && q.trim() && (
          <li className="customer-list-empty">No customers found.</li>
        )}
        {customers.map((c) => (
          <li key={c.id}>
            <button
              type="button"
              className={`customer-item ${selectedCustomerId === c.id ? 'selected' : ''}`}
              onClick={() => onSelectCustomer(c.id)}
            >
              <span className="customer-name">{c.name}</span>
              <span className="customer-meta">{c.code}{c.email ? ` · ${c.email}` : ''}</span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
