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
    <div>
      {/* Search bar */}
      <div className="ios-card" style={{ padding: '0.6rem 1rem', marginBottom: '0.75rem' }}>
        <input
          type="text"
          placeholder="Search products…"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && handleSearch()}
          className="ios-search"
          style={{ width: '100%' }}
          aria-label="Search products"
        />
      </div>

      {/* Category chips */}
      <div className="chip-row">
        <button
          type="button"
          className={`chip${categoryId === '' ? ' active' : ''}`}
          onClick={() => { setCategoryId(''); setPage(1); }}
        >
          All
        </button>
        {categories.map((c) => (
          <button
            key={c.id}
            type="button"
            className={`chip${categoryId === c.id ? ' active' : ''}`}
            onClick={() => { setCategoryId(c.id); setPage(1); }}
          >
            {c.name}
          </button>
        ))}
      </div>

      {/* Product grid */}
      <div className="catalog-grid">
        {(productList?.items ?? []).map((p) => (
          <div key={p.id} className="catalog-card">
            <div className="catalog-card-sku">{p.sku}</div>
            <div className="catalog-card-name">{p.name}</div>
            {p.description && (
              <div className="catalog-card-desc">{p.description}</div>
            )}
            <div className="catalog-card-footer">
              <button
                type="button"
                className="btn-add"
                onClick={() => onAddProduct(p, 1)}
                aria-label={`Add ${p.name} to cart`}
              >
                + Add
              </button>
            </div>
          </div>
        ))}
      </div>

      {/* Pagination */}
      {productList && productList.total > 0 && (
        <div className="catalog-pagination">
          <button
            type="button"
            className="btn-sm"
            disabled={page <= 1}
            onClick={() => setPage((p) => Math.max(1, p - 1))}
          >
            ← Prev
          </button>
          <span className="catalog-page-info">
            {page} / {totalPages}
          </span>
          <button
            type="button"
            className="btn-sm"
            disabled={page >= totalPages}
            onClick={() => setPage((p) => p + 1)}
          >
            Next →
          </button>
        </div>
      )}
    </div>
  );
}
