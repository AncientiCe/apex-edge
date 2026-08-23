//! The browser page: a paper-like render of whatever arrived on port 9100.
//!
//! The receipt is drawn twice — as selectable monospace text, and onto a canvas that
//! can be saved as a PNG. The canvas is what makes this a printer rather than a log
//! viewer: styling, alignment and paper width are all visible at a glance.

use axum::response::Html;

pub async fn page() -> Html<&'static str> {
    Html(PAGE_HTML)
}

const PAGE_HTML: &str = r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>ApexEdge Virtual Printer</title>
    <script src="https://cdn.tailwindcss.com"></script>
  </head>
  <body class="min-h-screen bg-slate-100 text-slate-900">
    <main class="mx-auto max-w-6xl p-6">
      <header class="mb-6 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 class="text-3xl font-bold tracking-tight">Virtual Printer</h1>
          <p class="text-sm text-slate-600">
            Listening on port 9100. Print to it from ApexEdge and the receipt appears here.
          </p>
        </div>
        <div class="flex gap-2">
          <button id="refresh" class="rounded border border-slate-300 bg-white px-3 py-2 text-sm hover:bg-slate-50">Refresh</button>
          <button id="clear" class="rounded border border-slate-300 bg-white px-3 py-2 text-sm hover:bg-slate-50">Clear</button>
        </div>
      </header>

      <div class="grid grid-cols-1 gap-6 lg:grid-cols-[20rem_1fr]">
        <section class="rounded-xl border border-slate-200 bg-white shadow-sm">
          <div class="border-b border-slate-200 px-4 py-3">
            <h2 class="text-lg font-semibold">Jobs</h2>
            <p id="meta" class="text-sm text-slate-500">Waiting for a print job...</p>
          </div>
          <ul id="jobs" class="max-h-[32rem] divide-y divide-slate-200 overflow-y-auto"></ul>
        </section>

        <section class="space-y-6">
          <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm">
            <div class="mb-3 flex items-center justify-between">
              <h2 class="text-lg font-semibold">Paper</h2>
              <button id="png" class="rounded bg-indigo-600 px-3 py-2 text-sm font-semibold text-white hover:bg-indigo-700">Save PNG</button>
            </div>
            <canvas id="paper" class="mx-auto block rounded border border-slate-200 bg-white"></canvas>
          </div>

          <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm">
            <h2 class="mb-3 text-lg font-semibold">Text</h2>
            <pre id="text" class="overflow-x-auto rounded bg-slate-50 p-3 text-xs leading-5"></pre>
          </div>

          <div class="rounded-xl border border-slate-200 bg-white p-4 shadow-sm">
            <h2 class="mb-3 text-lg font-semibold">Bytes</h2>
            <p id="warnings" class="mb-2 text-sm text-rose-600"></p>
            <pre id="hex" class="max-h-64 overflow-auto rounded bg-slate-50 p-3 text-[11px] leading-5"></pre>
          </div>
        </section>
      </div>
    </main>

    <script>
      const CHAR_W = 9;
      const LINE_H = 20;
      const PAD = 16;

      let jobs = [];
      let selected = null;

      const els = {
        jobs: document.getElementById('jobs'),
        meta: document.getElementById('meta'),
        text: document.getElementById('text'),
        hex: document.getElementById('hex'),
        warnings: document.getElementById('warnings'),
        paper: document.getElementById('paper'),
      };

      function paperWidth(job) {
        // The encoder does the wrapping, so the longest line is the paper width.
        return job.elements.reduce(
          (widest, el) => (el.kind === 'line' ? Math.max(widest, el.text.length) : widest),
          32,
        );
      }

      function draw(job) {
        const canvas = els.paper;
        const cols = paperWidth(job);
        const rows = job.elements.reduce((n, el) => {
          if (el.kind === 'line') return n + 1;
          if (el.kind === 'feed') return n + el.lines;
          if (el.kind === 'qr_code' || el.kind === 'barcode') return n + 4;
          return n + 1;
        }, 1);

        const ratio = window.devicePixelRatio || 1;
        canvas.width = (cols * CHAR_W + PAD * 2) * ratio;
        canvas.height = (rows * LINE_H + PAD * 2) * ratio;
        canvas.style.width = cols * CHAR_W + PAD * 2 + 'px';
        canvas.style.height = rows * LINE_H + PAD * 2 + 'px';

        const ctx = canvas.getContext('2d');
        ctx.scale(ratio, ratio);
        ctx.fillStyle = '#ffffff';
        ctx.fillRect(0, 0, canvas.width, canvas.height);
        ctx.fillStyle = '#111827';
        ctx.textBaseline = 'top';

        const paper = cols * CHAR_W;
        let y = PAD;

        for (const el of job.elements) {
          if (el.kind === 'line') {
            const style = el.style || {};
            const scaleX = style.double_width ? 2 : 1;
            const scaleY = style.double_height ? 2 : 1;
            const size = 14 * scaleY;
            ctx.font = `${style.bold ? '700 ' : ''}${size}px ui-monospace, Menlo, Consolas, monospace`;
            const width = el.text.length * CHAR_W * scaleX;
            let x = PAD;
            if (style.align === 'centre') x = PAD + (paper - width) / 2;
            if (style.align === 'right') x = PAD + paper - width;
            ctx.save();
            ctx.translate(x, y);
            ctx.scale(scaleX, 1);
            ctx.fillText(el.text, 0, 0);
            ctx.restore();
            y += LINE_H * scaleY;
          } else if (el.kind === 'feed') {
            y += LINE_H * el.lines;
          } else if (el.kind === 'barcode' || el.kind === 'qr_code') {
            // The payload is what matters for verification; a rendered symbol would
            // only prove the browser can draw squares.
            const label = el.kind === 'barcode' ? `${el.symbology}: ${el.data}` : `QR: ${el.data}`;
            const boxW = Math.min(paper, 220);
            const x = PAD + (paper - boxW) / 2;
            ctx.strokeStyle = '#94a3b8';
            ctx.setLineDash([4, 3]);
            ctx.strokeRect(x, y + 4, boxW, LINE_H * 2.5);
            ctx.setLineDash([]);
            ctx.font = '11px ui-monospace, Menlo, Consolas, monospace';
            ctx.fillText(label.slice(0, Math.floor(boxW / 6)), x + 6, y + LINE_H);
            y += LINE_H * 4;
          } else if (el.kind === 'cut') {
            ctx.strokeStyle = '#cbd5e1';
            ctx.setLineDash([6, 4]);
            ctx.beginPath();
            ctx.moveTo(PAD, y + LINE_H / 2);
            ctx.lineTo(PAD + paper, y + LINE_H / 2);
            ctx.stroke();
            ctx.setLineDash([]);
            y += LINE_H;
          } else if (el.kind === 'drawer_kick') {
            ctx.font = 'italic 12px ui-monospace, Menlo, Consolas, monospace';
            ctx.fillText('[cash drawer opened]', PAD, y);
            y += LINE_H;
          }
        }
      }

      function select(id) {
        selected = jobs.find((job) => job.id === id) || null;
        renderList();
        if (!selected) {
          els.text.textContent = '';
          els.hex.textContent = '';
          els.warnings.textContent = '';
          return;
        }
        els.text.textContent = selected.text;
        els.hex.textContent = selected.hex;
        els.warnings.textContent = selected.warnings.join(' | ');
        draw(selected);
      }

      function renderList() {
        els.jobs.innerHTML = jobs
          .slice()
          .reverse()
          .map((job) => {
            const active = selected && selected.id === job.id;
            const first = (job.text.split('\n').find((l) => l.trim().length) || '(no text)').trim();
            return `
              <li>
                <button data-id="${job.id}" class="w-full px-4 py-3 text-left hover:bg-slate-50 ${active ? 'bg-indigo-50' : ''}">
                  <span class="block text-sm font-semibold">#${job.id} &middot; ${job.dialect}</span>
                  <span class="block truncate text-xs text-slate-500">${first}</span>
                  <span class="block text-xs text-slate-400">${job.bytes} bytes &middot; ${new Date(job.received_at).toLocaleTimeString()}</span>
                </button>
              </li>`;
          })
          .join('');
        els.jobs.querySelectorAll('button[data-id]').forEach((button) => {
          button.addEventListener('click', () => select(Number(button.dataset.id)));
        });
      }

      async function load() {
        const res = await fetch('/api/jobs');
        const data = await res.json();
        jobs = data.jobs || [];
        els.meta.textContent = jobs.length
          ? `${jobs.length} job(s) received`
          : 'Waiting for a print job...';
        const keep = selected && jobs.some((job) => job.id === selected.id);
        select(keep ? selected.id : jobs.length ? jobs[jobs.length - 1].id : null);
      }

      document.getElementById('refresh').addEventListener('click', load);
      document.getElementById('clear').addEventListener('click', async () => {
        await fetch('/api/jobs', { method: 'DELETE' });
        selected = null;
        load();
      });
      document.getElementById('png').addEventListener('click', () => {
        if (!selected) return;
        const link = document.createElement('a');
        link.download = `receipt-${selected.id}.png`;
        link.href = els.paper.toDataURL('image/png');
        link.click();
      });

      load();
      setInterval(load, 2000);
    </script>
  </body>
</html>
"#;
