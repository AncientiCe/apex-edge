import { useCallback, useEffect, useState } from 'react';
import type { CategoryResult, ProductSearchResult, ProductListResponse } from '../api/types';

interface Props {
  baseUrl: string;
  categories: CategoryResult[];
  productList: ProductListResponse | null;
  onLoadCategories: () => void;
  onLoadProducts: (params: { q?: string; category_id?: string; page: number }) => void;
  onAddProduct: (product: ProductSearchResult, quantity: number) => void;
}

export function CatalogPanel({
  baseUrl,
  categories,
  productList,
  onLoadCategories,
  onLoadProducts,
  onAddProduct,
}: Props) {
  const [q, setQ] = useState('');
  const [submittedQ, setSubmittedQ] = useState('');
  const [categoryId, setCategoryId] = useState<string>('');
  const [page, setPage] = useState(1);
  const [quantity, setQuantity] = useState(1);

  useEffect(() => {
    if (!baseUrl) return;
    onLoadCategories();
  }, [baseUrl, onLoadCategories]);

  useEffect(() => {
    if (!baseUrl) return;
    onLoadProducts({
      q: submittedQ.trim() || undefined,
      category_id: categoryId || undefined,
      page,
    });
  }, [baseUrl, categoryId, page, submittedQ, onLoadProducts]);

  const totalPages = productList
    ? Math.max(1, Math.ceil(productList.total / productList.per_page))
    : 0;

  const handleSearch = useCallback(() => {
    setSubmittedQ(q);
    setPage(1);
  }, [q]);

  return (
    <section className="panel catalog-panel">
      <h2>Catalog</h2>
      <div className="catalog-filters">
        <input
          type="text"
          placeholder="Search by SKU, name, or description…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
          className="catalog-search"
          aria-label="Search products"
        />
        <button type="button" onClick={handleSearch}>
          Search
        </button>
        <select
          value={categoryId}
          onChange={(e) => { setCategoryId(e.target.value); setPage(1); }}
          aria-label="Category"
        >
          <option value="">All categories</option>
          {categories.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </select>
      </div>
      <div className="catalog-grid">
        {(productList?.items ?? []).map((p) => (
          <div key={p.id} className="catalog-card">
            <div className="catalog-card-body">
              <div className="catalog-card-sku">{p.sku}</div>
              <div className="catalog-card-name">{p.name}</div>
              {p.description && (
                <div className="catalog-card-desc">{p.description}</div>
              )}
            </div>
            <div className="catalog-card-actions">
              <input
                type="number"
                min={1}
                value={quantity}
                onChange={(e) => setQuantity(Math.max(1, parseInt(e.target.value, 10) || 1))}
                onClick={(e) => e.stopPropagation()}
                aria-label={`Quantity for ${p.name}`}
              />
              <button
                type="button"
                onClick={() => onAddProduct(p, quantity)}
                className="btn-add"
              >
                Add to cart
              </button>
            </div>
          </div>
        ))}
      </div>
      {productList && productList.total > 0 && (
        <div className="catalog-pagination">
          <button
            type="button"
            disabled={page <= 1}
            onClick={() => setPage((p) => Math.max(1, p - 1))}
          >
            Previous
          </button>
          <span className="catalog-page-info">
            Page {page} of {totalPages} ({productList.total} total)
          </span>
          <button
            type="button"
            disabled={page >= totalPages}
            onClick={() => setPage((p) => p + 1)}
          >
            Next
          </button>
        </div>
      )}
    </section>
  );
}
