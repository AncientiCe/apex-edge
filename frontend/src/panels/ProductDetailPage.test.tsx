import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { ProductDetailPage } from './ProductDetailPage';
import type { ProductSearchResult } from '../api/types';

function makeProduct(overrides: Partial<ProductSearchResult> = {}): ProductSearchResult {
  return {
    id: 'aaaaaaaa-0000-0000-0000-000000000001',
    sku: 'TEST-001',
    name: 'Test Product',
    category_id: 'cat-1',
    tax_category_id: 'tax-1',
    description: 'A great product',
    is_active: true,
    available_qty: 10,
    image_urls: [
      'https://example.com/img1.jpg',
      'https://example.com/img2.jpg',
      'https://example.com/img3.jpg',
    ],
    ...overrides,
  };
}

function renderPdp(
  product: ProductSearchResult | null,
  onAddProduct = vi.fn()
) {
  return render(
    <MemoryRouter initialEntries={[`/product/${product?.id ?? 'unknown'}`]}>
      <Routes>
        <Route
          path="/product/:id"
          element={
            <ProductDetailPage
              product={product}
              onAddProduct={onAddProduct}
              onBack={vi.fn()}
            />
          }
        />
      </Routes>
    </MemoryRouter>
  );
}

describe('ProductDetailPage', () => {
  it('renders product name and description', () => {
    renderPdp(makeProduct());
    expect(screen.getByText('Test Product')).toBeInTheDocument();
    expect(screen.getByText('A great product')).toBeInTheDocument();
  });

  it('renders the first image as the selected gallery image', () => {
    renderPdp(makeProduct());
    const mainImg = screen.getByRole('img', { name: /Test Product/i });
    expect(mainImg).toHaveAttribute('src', 'https://example.com/img1.jpg');
  });

  it('shows thumbnail strip with all images', () => {
    renderPdp(makeProduct());
    const thumbnails = screen.getAllByRole('img', { name: /thumbnail/i });
    expect(thumbnails).toHaveLength(3);
  });

  it('clicking a thumbnail changes the selected image', () => {
    renderPdp(makeProduct());
    const thumbnails = screen.getAllByRole('img', { name: /thumbnail/i });
    fireEvent.click(thumbnails[1]);
    const mainImg = screen.getByRole('img', { name: /Test Product/i });
    expect(mainImg).toHaveAttribute('src', 'https://example.com/img2.jpg');
  });

  it('shows available quantity badge when stock is tracked', () => {
    renderPdp(makeProduct({ available_qty: 7 }));
    expect(screen.getByText(/7.*in stock/i)).toBeTruthy();
  });

  it('shows Out of Stock badge when available_qty is 0', () => {
    renderPdp(makeProduct({ available_qty: 0 }));
    expect(screen.getByText(/out of stock/i)).toBeTruthy();
  });

  it('shows Out of Stock badge when is_active is false', () => {
    renderPdp(makeProduct({ is_active: false }));
    expect(screen.getByText(/out of stock/i)).toBeTruthy();
  });

  it('shows untracked badge when available_qty is null', () => {
    renderPdp(makeProduct({ available_qty: null }));
    expect(screen.getByText(/available/i)).toBeTruthy();
  });

  it('quantity stepper starts at 1', () => {
    renderPdp(makeProduct());
    const input = screen.getByRole('spinbutton');
    expect((input as HTMLInputElement).value).toBe('1');
  });

  it('quantity stepper increments', () => {
    renderPdp(makeProduct());
    const inc = screen.getByRole('button', { name: /\+/i });
    fireEvent.click(inc);
    const input = screen.getByRole('spinbutton');
    expect((input as HTMLInputElement).value).toBe('2');
  });

  it('quantity stepper decrements but not below 1', () => {
    renderPdp(makeProduct());
    const dec = screen.getByRole('button', { name: /−/i });
    fireEvent.click(dec);
    const input = screen.getByRole('spinbutton');
    expect((input as HTMLInputElement).value).toBe('1');
  });

  it('add to cart button is disabled when out of stock', () => {
    renderPdp(makeProduct({ available_qty: 0 }));
    const btn = screen.getByTestId('add-to-cart-btn');
    expect(btn).toHaveProperty('disabled', true);
  });

  it('add to cart button is disabled when is_active is false', () => {
    renderPdp(makeProduct({ is_active: false }));
    const btn = screen.getByTestId('add-to-cart-btn');
    expect(btn).toHaveProperty('disabled', true);
  });

  it('add to cart calls onAddProduct with product and quantity', () => {
    const onAdd = vi.fn();
    renderPdp(makeProduct(), onAdd);
    const inc = screen.getByRole('button', { name: /\+/i });
    fireEvent.click(inc);
    fireEvent.click(inc);
    const btn = screen.getByTestId('add-to-cart-btn');
    fireEvent.click(btn);
    expect(onAdd).toHaveBeenCalledWith(expect.objectContaining({ id: 'aaaaaaaa-0000-0000-0000-000000000001' }), 3);
  });

  it('shows loading state when product is null', () => {
    renderPdp(null);
    expect(screen.getByText(/loading/i)).toBeTruthy();
  });
});
