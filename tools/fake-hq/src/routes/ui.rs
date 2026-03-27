use std::sync::Arc;

use axum::{extract::Path, response::Html, routing::get, Json, Router};

use crate::AppState;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(index))
        .route("/orders/:submission_id", get(detail))
}

pub async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({"ok": true}))
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn detail(Path(submission_id): Path<String>) -> Html<String> {
    Html(DETAIL_HTML.replace("__SUBMISSION_ID__", &submission_id))
}

const INDEX_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Fake HQ - Orders</title>
    <script src="https://cdn.tailwindcss.com"></script>
  </head>
  <body class="min-h-screen bg-slate-100 text-slate-900">
    <main class="mx-auto max-w-7xl p-6">
      <header class="mb-6 flex items-center justify-between">
        <div>
          <h1 class="text-3xl font-bold tracking-tight">Fake HQ Orders</h1>
          <p class="text-sm text-slate-600">Local OMS dashboard for demos and sync testing.</p>
        </div>
        <a href="/health" class="rounded bg-emerald-600 px-3 py-2 text-sm font-semibold text-white hover:bg-emerald-700">Health</a>
      </header>
      <section class="overflow-hidden rounded-xl border border-slate-200 bg-white shadow-sm">
        <div class="border-b border-slate-200 px-4 py-3">
          <h2 class="text-lg font-semibold">Received Orders</h2>
          <p id="meta" class="text-sm text-slate-500">Loading...</p>
        </div>
        <div class="overflow-x-auto">
          <table class="min-w-full divide-y divide-slate-200">
            <thead class="bg-slate-50">
              <tr>
                <th class="px-4 py-3 text-left text-xs font-semibold uppercase text-slate-600">Submission</th>
                <th class="px-4 py-3 text-left text-xs font-semibold uppercase text-slate-600">Order</th>
                <th class="px-4 py-3 text-left text-xs font-semibold uppercase text-slate-600">Store</th>
                <th class="px-4 py-3 text-right text-xs font-semibold uppercase text-slate-600">Items</th>
                <th class="px-4 py-3 text-right text-xs font-semibold uppercase text-slate-600">Total</th>
                <th class="px-4 py-3 text-left text-xs font-semibold uppercase text-slate-600">Payments</th>
                <th class="px-4 py-3 text-left text-xs font-semibold uppercase text-slate-600">Submitted</th>
              </tr>
            </thead>
            <tbody id="rows" class="divide-y divide-slate-200 bg-white"></tbody>
          </table>
        </div>
        <div class="flex items-center justify-between border-t border-slate-200 px-4 py-3">
          <button id="prev" class="rounded border border-slate-300 px-3 py-2 text-sm hover:bg-slate-50">Previous</button>
          <span id="pageLabel" class="text-sm text-slate-600"></span>
          <button id="next" class="rounded border border-slate-300 px-3 py-2 text-sm hover:bg-slate-50">Next</button>
        </div>
      </section>
    </main>
    <script>
      let page = 1;
      const perPage = 20;
      let lastTotal = 0;

      const rows = document.getElementById('rows');
      const pageLabel = document.getElementById('pageLabel');
      const meta = document.getElementById('meta');
      const prev = document.getElementById('prev');
      const next = document.getElementById('next');

      function money(cents) {
        return '$' + (Number(cents) / 100).toFixed(2);
      }

      function shortId(value) {
        return String(value).slice(0, 8);
      }

      async function load() {
        const res = await fetch(`/api/orders?page=${page}&per_page=${perPage}`);
        const data = await res.json();
        const items = data.items || [];
        lastTotal = Number(data.total || 0);
        const totalPages = Math.max(1, Math.ceil(lastTotal / perPage));
        if (page > totalPages) {
          page = totalPages;
        }
        rows.innerHTML = items.map((item) => `
          <tr class="hover:bg-slate-50">
            <td class="px-4 py-3 text-sm font-medium">
              <a class="text-indigo-600 hover:text-indigo-800" href="/orders/${item.submission_id}">
                ${shortId(item.submission_id)}
              </a>
            </td>
            <td class="px-4 py-3 text-sm text-slate-700">${shortId(item.order_id)}</td>
            <td class="px-4 py-3 text-sm text-slate-700">${shortId(item.store_id)}</td>
            <td class="px-4 py-3 text-right text-sm text-slate-700">${item.line_count}</td>
            <td class="px-4 py-3 text-right text-sm font-semibold text-slate-900">${money(item.total_cents)}</td>
            <td class="px-4 py-3 text-sm text-slate-700">${item.payment_summary}</td>
            <td class="px-4 py-3 text-sm text-slate-700">${new Date(item.submitted_at).toLocaleString()}</td>
          </tr>
        `).join('');
        meta.textContent = `Total orders: ${lastTotal}`;
        pageLabel.textContent = `Page ${page} of ${totalPages}`;
        prev.disabled = page <= 1;
        next.disabled = page >= totalPages;
      }

      prev.addEventListener('click', () => {
        if (page > 1) {
          page -= 1;
          load();
        }
      });

      next.addEventListener('click', () => {
        const totalPages = Math.max(1, Math.ceil(lastTotal / perPage));
        if (page < totalPages) {
          page += 1;
          load();
        }
      });

      load();
    </script>
  </body>
</html>
"#;

const DETAIL_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Fake HQ - Order Detail</title>
    <script src="https://cdn.tailwindcss.com"></script>
  </head>
  <body class="min-h-screen bg-slate-100 text-slate-900">
    <main class="mx-auto max-w-5xl p-6">
      <header class="mb-6 flex items-center justify-between">
        <div>
          <h1 class="text-3xl font-bold tracking-tight">Order Detail</h1>
          <p id="subId" class="text-sm text-slate-600">Submission: __SUBMISSION_ID__</p>
        </div>
        <a href="/" class="rounded border border-slate-300 px-3 py-2 text-sm hover:bg-slate-50">Back</a>
      </header>
      <section class="mb-6 rounded-xl border border-slate-200 bg-white p-4 shadow-sm">
        <dl class="grid grid-cols-1 gap-4 md:grid-cols-2">
          <div><dt class="text-xs uppercase text-slate-500">Order ID</dt><dd id="orderId" class="font-medium"></dd></div>
          <div><dt class="text-xs uppercase text-slate-500">Store</dt><dd id="storeId" class="font-medium"></dd></div>
          <div><dt class="text-xs uppercase text-slate-500">Register</dt><dd id="registerId" class="font-medium"></dd></div>
          <div><dt class="text-xs uppercase text-slate-500">Submitted</dt><dd id="submittedAt" class="font-medium"></dd></div>
          <div><dt class="text-xs uppercase text-slate-500">Total</dt><dd id="total" class="font-medium"></dd></div>
          <div><dt class="text-xs uppercase text-slate-500">Discount/Tax</dt><dd id="discountTax" class="font-medium"></dd></div>
        </dl>
      </section>
      <section class="mb-6 overflow-hidden rounded-xl border border-slate-200 bg-white shadow-sm">
        <div class="border-b border-slate-200 px-4 py-3">
          <h2 class="text-lg font-semibold">Line Items</h2>
        </div>
        <div class="overflow-x-auto">
          <table class="min-w-full divide-y divide-slate-200">
            <thead class="bg-slate-50">
              <tr>
                <th class="px-4 py-3 text-left text-xs font-semibold uppercase text-slate-600">SKU</th>
                <th class="px-4 py-3 text-left text-xs font-semibold uppercase text-slate-600">Name</th>
                <th class="px-4 py-3 text-right text-xs font-semibold uppercase text-slate-600">Qty</th>
                <th class="px-4 py-3 text-right text-xs font-semibold uppercase text-slate-600">Unit</th>
                <th class="px-4 py-3 text-right text-xs font-semibold uppercase text-slate-600">Line Total</th>
                <th class="px-4 py-3 text-right text-xs font-semibold uppercase text-slate-600">Discount</th>
                <th class="px-4 py-3 text-right text-xs font-semibold uppercase text-slate-600">Tax</th>
              </tr>
            </thead>
            <tbody id="lineRows" class="divide-y divide-slate-200 bg-white"></tbody>
          </table>
        </div>
      </section>
      <section class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm">
        <h2 class="mb-3 text-lg font-semibold">Payments</h2>
        <ul id="payments" class="list-disc pl-5 text-sm text-slate-700"></ul>
      </section>
    </main>
    <script>
      const submissionId = '__SUBMISSION_ID__';
      const money = (cents) => '$' + (Number(cents) / 100).toFixed(2);

      async function load() {
        const res = await fetch(`/api/orders/${submissionId}`);
        if (res.status !== 200) {
          document.body.innerHTML = '<main class="p-6"><h1 class="text-2xl font-bold">Order not found</h1><a class="text-indigo-600" href="/">Back to list</a></main>';
          return;
        }
        const data = await res.json();
        const payload = data.payload || {};
        const order = payload.order || {};
        document.getElementById('orderId').textContent = String(order.order_id || '');
        document.getElementById('storeId').textContent = String(payload.store_id || data.store_id || '');
        document.getElementById('registerId').textContent = String(payload.register_id || data.register_id || '');
        document.getElementById('submittedAt').textContent = new Date(data.submitted_at).toLocaleString();
        document.getElementById('total').textContent = money(order.total_cents || 0);
        document.getElementById('discountTax').textContent = `${money(order.discount_cents || 0)} / ${money(order.tax_cents || 0)}`;

        const lines = order.lines || [];
        document.getElementById('lineRows').innerHTML = lines.map((line) => `
          <tr>
            <td class="px-4 py-3 text-sm">${line.sku || ''}</td>
            <td class="px-4 py-3 text-sm">${line.name || ''}</td>
            <td class="px-4 py-3 text-right text-sm">${line.quantity || 0}</td>
            <td class="px-4 py-3 text-right text-sm">${money(line.unit_price_cents || 0)}</td>
            <td class="px-4 py-3 text-right text-sm">${money(line.line_total_cents || 0)}</td>
            <td class="px-4 py-3 text-right text-sm">${money(line.discount_cents || 0)}</td>
            <td class="px-4 py-3 text-right text-sm">${money(line.tax_cents || 0)}</td>
          </tr>
        `).join('');

        const payments = order.payments || [];
        document.getElementById('payments').innerHTML = payments
          .map((p) => `<li>${p.external_reference || 'payment'} - ${money(p.amount_cents || 0)}</li>`)
          .join('');
      }

      load();
    </script>
  </body>
</html>
"#;
