import { createServer, type ServerResponse } from "node:http";
import { fileURLToPath } from "node:url";

import { EkgClient, StdioTransport } from "@ekg/sdk";

const port = Number.parseInt(process.env.EKG_INSPECTOR_PORT ?? "4317", 10);
const transport = StdioTransport.spawn(
  process.env.EKG_SERVER ??
    fileURLToPath(new URL("../../../target/debug/ekg-server", import.meta.url)),
);
const client = new EkgClient(transport, {
  adminToken: process.env.EKG_ADMIN_TOKEN,
});

const server = createServer(async (request, response) => {
  try {
    const pathname = new URL(request.url ?? "/", "http://localhost").pathname;
    if (pathname === "/") return html(response);
    if (pathname === "/api/metrics") return json(response, await client.metricsSnapshot());
    if (pathname === "/api/concepts") return json(response, await client.listConcepts());
    if (pathname === "/api/procedures") return json(response, await client.listProcedures());
    if (pathname === "/api/episodes") return json(response, await client.listEpisodes({ limit: 50 }));
    response.writeHead(404, { "content-type": "application/json" });
    response.end(JSON.stringify({ error: "not found" }));
  } catch (error) {
    json(response, { error: error instanceof Error ? error.message : String(error) }, 500);
  }
});

server.listen(port, "127.0.0.1", () => {
  console.log(`EKG Inspector listening at http://127.0.0.1:${port}`);
});

function json(response: ServerResponse, value: unknown, status = 200): void {
  response.writeHead(status, { "content-type": "application/json; charset=utf-8" });
  response.end(JSON.stringify(value));
}

function html(response: ServerResponse): void {
  response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
  response.end(INDEX_HTML);
}

const INDEX_HTML = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>EKG Inspector</title>
  <style>
    :root { color-scheme: dark; font-family: ui-sans-serif, system-ui, sans-serif; background: #101114; color: #eceef2; }
    body { max-width: 1180px; margin: 0 auto; padding: 32px 20px 60px; }
    h1 { margin: 0 0 8px; letter-spacing: -.03em; } h2 { margin-top: 32px; }
    .muted { color: #9aa0ad; } .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(180px, 1fr)); gap: 12px; }
    .card, table { background: #191b21; border: 1px solid #2b2e38; border-radius: 12px; }
    .card { padding: 16px; } .value { display:block; font-size: 28px; font-weight: 700; margin-top: 6px; }
    table { width: 100%; border-collapse: separate; border-spacing: 0; overflow: hidden; } th, td { text-align:left; padding: 10px 12px; border-bottom: 1px solid #2b2e38; vertical-align: top; } tr:last-child td { border-bottom: 0; }
    th { color: #9aa0ad; font-size: 12px; text-transform: uppercase; letter-spacing: .08em; } code { color: #b9c6ff; }
    .error { color: #ff9b9b; } button { background: #b9c6ff; color: #101114; border: 0; border-radius: 8px; padding: 8px 12px; cursor: pointer; }
  </style>
</head>
<body>
  <h1>EKG Inspector</h1>
  <p class="muted">A local read-only view of the graph, episodes, and flywheel metrics.</p>
  <button id="refresh">Refresh</button>
  <p id="status" class="muted"></p>
  <section><h2>Metrics</h2><div id="metrics" class="grid"></div></section>
  <section><h2>Knowledge</h2><div id="knowledge"></div></section>
  <section><h2>Recent episodes</h2><div id="episodes"></div></section>
  <script>
    const $ = (id) => document.getElementById(id);
    const esc = (value) => String(value ?? '').replace(/[&<>"']/g, (c) => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
    const get = async (path) => { const r = await fetch(path); if (!r.ok) throw new Error(await r.text()); return r.json(); };
    const cell = (value) => '<code>' + esc(typeof value === 'object' ? JSON.stringify(value) : value) + '</code>';
    async function refresh() {
      $('status').textContent = 'Refreshing…';
      try {
        const [metrics, concepts, procedures, episodes] = await Promise.all([get('/api/metrics'), get('/api/concepts'), get('/api/procedures'), get('/api/episodes')]);
        const i = metrics.intuition;
        $('metrics').innerHTML = [['Episodes', metrics.episodeCount], ['Indexed docs', i.indexedDocuments], ['Recall queries', i.retrievalQueries], ['Candidates examined', i.candidatesExamined], ['Ranking examples', i.rankingExamples], ['Grounded tasks', i.groundedTasks]].map(([label, value]) => '<div class="card"><span class="muted">'+label+'</span><span class="value">'+esc(value)+'</span></div>').join('');
        $('knowledge').innerHTML = '<table><thead><tr><th>Type</th><th>Name</th><th>Id</th></tr></thead><tbody>' + concepts.map((c) => '<tr><td>Concept</td><td>'+esc(c.name)+'</td><td>'+cell(c.id)+'</td></tr>').concat(procedures.map((p) => '<tr><td>Procedure</td><td>'+esc(p.name)+'</td><td>'+cell(p.id)+'</td></tr>')).join('') + '</tbody></table>';
        $('episodes').innerHTML = '<table><thead><tr><th>Situation</th><th>Disposition</th><th>Rung</th><th>Result</th></tr></thead><tbody>' + episodes.map((e) => '<tr><td>'+esc(e.situation)+'</td><td>'+cell(e.evaluation?.success === true ? 'success' : e.evaluation?.success === false ? 'failure' : 'unverified')+'</td><td>'+cell(e.cost?.rungReached ?? e.cost?.rung_reached)+'</td><td>'+cell(e.observedResult ?? e.observed_result)+'</td></tr>').join('') + '</tbody></table>';
        $('status').textContent = 'Updated ' + new Date().toLocaleTimeString();
      } catch (error) { $('status').innerHTML = '<span class="error">'+esc(error.message || error)+'</span>'; }
    }
    $('refresh').addEventListener('click', refresh); refresh();
  </script>
</body>
</html>`;

process.once("SIGINT", () => { client.close(); server.close(); });
process.once("SIGTERM", () => { client.close(); server.close(); });
