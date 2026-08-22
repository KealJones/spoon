import { createServer, type Server, type ServerResponse } from "node:http";
import { fileURLToPath } from "node:url";

import { EkgClient, StdioTransport } from "@ekg/sdk";

type JsonRecord = Record<string, unknown>;

interface InspectorClient {
  metricsSnapshot(): Promise<unknown>;
  listConcepts(): Promise<unknown>;
  listProcedures(): Promise<unknown>;
  listEpisodes(filter: { limit: number }): Promise<unknown>;
  getEpisode(episodeId: string): Promise<unknown>;
}

export interface EpisodeDetail {
  narrative: JsonRecord;
  raw: unknown;
}

const REDACTED = "[REDACTED]";
const secretKey =
  /(?:api[_-]?key|authorization|bearer|cookie|credential|pass(?:word)?|secret|session|token|private[_-]?key|environment|env(?:ironment)?)/i;
const secretValue =
  /(?:bearer\s+|-----begin [^-]+ private key-----|(?:api[_-]?key|token|secret|password)\s*[:=])/i;

export function redactSensitive(value: unknown, key = ""): unknown {
  if (secretKey.test(key)) return REDACTED;
  if (typeof value === "string")
    return secretValue.test(value) ? REDACTED : value;
  if (Array.isArray(value)) return value.map((item) => redactSensitive(item));
  if (!isRecord(value)) return value;
  return Object.fromEntries(
    Object.entries(value).map(([childKey, childValue]) => [
      childKey,
      redactSensitive(childValue, childKey),
    ]),
  );
}

/** A read-only, redacted projection suitable for both the API and the web view. */
export function episodeDetail(value: unknown): EpisodeDetail {
  const redacted = redactSensitive(value);
  const episode = isRecord(redacted) ? redacted : {};
  const teacher = recordAt(
    episode,
    "teacher_interaction",
    "teacherInteraction",
  );
  const provenance = teacher
    ? (recordAt(teacher, "provenance") ?? teacher)
    : undefined;
  const validation = teacher ? recordAt(teacher, "validation") : undefined;
  const evaluation = recordAt(episode, "evaluation");
  const cost = recordAt(episode, "cost") ?? {};
  const action = stringAt(episode, "action");
  const proposedContent = teacher?.content;
  const observed = valueAt(episode, "observed_result", "observedResult");
  const teacherError = stringAt(teacher, "providerError", "provider_error");
  const rung = valueAt(cost, "rung_reached", "rungReached");
  const capability = recordAt(
    episode,
    "capability",
    "capability_interaction",
    "capabilityInteraction",
  );
  const abstention =
    stringAt(episode, "abstention_reason", "abstentionReason") ??
    (String(rung ?? "").toLowerCase() === "abstain"
      ? (teacherError ?? action)
      : undefined);

  return {
    narrative: compact({
      id: stringAt(episode, "id"),
      request: stringAt(episode, "situation") ?? "Unknown request",
      escalation: compact({
        rung,
        stepsTaken: valueAt(cost, "steps_taken", "stepsTaken"),
        executionTrace: summarizeTrace(
          episode.execution_trace ?? episode.executionTrace,
        ),
      }),
      teacher: compact({
        used: Boolean(teacher),
        provider: stringAt(provenance, "provider"),
        model: stringAt(provenance, "model"),
        source: stringAt(teacher, "source") ?? stringAt(provenance, "teacher"),
        proposal: proposedContent,
        proposalSummary: summarizeProposal(proposedContent),
        providerError: teacherError,
        validation:
          validation &&
          compact({
            status: stringAt(validation, "status"),
            validatedAt: stringAt(validation, "validatedAt", "validated_at"),
            checks: Array.isArray(validation.checks)
              ? validation.checks.map((check) => summarizeCheck(check))
              : undefined,
          }),
      }),
      learning: compact({
        action: action ?? "no reusable procedure",
        summary: learningSummary(action),
        procedures: proposedNames(proposedContent, "procedures"),
        concepts: proposedNames(proposedContent, "concepts"),
        provenanceEpisode: stringAt(
          episode,
          "source_episode",
          "sourceEpisode",
          "learned_from",
          "learnedFrom",
        ),
      }),
      prediction: episode.prediction,
      observation: observed,
      evaluation:
        evaluation &&
        compact({
          tier: stringAt(evaluation, "tier"),
          success: evaluation.success,
          details: stringAt(evaluation, "details"),
          surprise: evaluation.surprise,
        }),
      cost: compact({
        rung,
        stepsTaken: valueAt(cost, "steps_taken", "stepsTaken"),
        budgetSpent: valueAt(cost, "budget_spent", "budgetSpent"),
      }),
      abstentionReason: abstention,
      capability:
        capability &&
        compact({
          name: stringAt(capability, "name"),
          permissions: capability.permissions,
          effects: capability.effects,
          locallyValidated: valueAt(
            capability,
            "locally_validated",
            "locallyValidated",
          ),
        }),
    }),
    raw: episode,
  };
}

export function createInspectorServer(client: InspectorClient): Server {
  return createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? "/", "http://localhost");
      if (url.pathname === "/") return html(response);
      if (url.pathname === "/api/metrics")
        return json(response, await client.metricsSnapshot());
      if (url.pathname === "/api/concepts")
        return json(response, await client.listConcepts());
      if (url.pathname === "/api/procedures")
        return json(response, await client.listProcedures());
      if (url.pathname === "/api/episodes")
        return json(response, await client.listEpisodes({ limit: 50 }));
      const episodeId = episodeIdFromPath(url.pathname);
      if (episodeId)
        return json(
          response,
          episodeDetail(await client.getEpisode(episodeId)),
        );
      json(response, { error: "not found" }, 404);
    } catch (error) {
      json(
        response,
        { error: error instanceof Error ? error.message : String(error) },
        500,
      );
    }
  });
}

function episodeIdFromPath(pathname: string): string | undefined {
  const match = /^\/api\/episodes\/([^/]+)$/.exec(pathname);
  return match ? decodeURIComponent(match[1]!) : undefined;
}

function json(response: ServerResponse, value: unknown, status = 200): void {
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
  });
  response.end(JSON.stringify(value));
}

function html(response: ServerResponse): void {
  response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
  response.end(inspectorHtml());
}

export function inspectorHtml(): string {
  return INDEX_HTML;
}

function isRecord(value: unknown): value is JsonRecord {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function recordAt(
  record: JsonRecord | undefined,
  ...keys: string[]
): JsonRecord | undefined {
  for (const key of keys)
    if (isRecord(record?.[key])) return record[key] as JsonRecord;
  return undefined;
}

function valueAt(record: JsonRecord | undefined, ...keys: string[]): unknown {
  for (const key of keys) if (record?.[key] !== undefined) return record[key];
  return undefined;
}

function stringAt(
  record: JsonRecord | undefined,
  ...keys: string[]
): string | undefined {
  const value = valueAt(record, ...keys);
  return typeof value === "string" || typeof value === "number"
    ? String(value)
    : undefined;
}

function compact(record: JsonRecord): JsonRecord {
  return Object.fromEntries(
    Object.entries(record).filter(([, value]) => value !== undefined),
  );
}

function summarizeTrace(trace: unknown): string[] | undefined {
  if (!Array.isArray(trace)) return undefined;
  return trace.slice(0, 12).map((step) => {
    if (typeof step === "string") return step;
    if (!isRecord(step)) return JSON.stringify(step);
    return (
      stringAt(step, "description", "action", "name", "message") ??
      JSON.stringify(step)
    );
  });
}

function summarizeProposal(content: unknown): string | undefined {
  if (!isRecord(content))
    return content === undefined
      ? undefined
      : "Teacher returned a direct result.";
  const kind = stringAt(content, "proposalKind", "proposal_kind");
  const answer = valueAt(content, "answer");
  const lesson = recordAt(content, "lesson");
  const names = [
    ...new Set([
      ...proposedNames(content, "procedures"),
      ...(lesson ? proposedNames(lesson, "procedures") : []),
    ]),
  ];
  if (kind)
    return `${kind.replaceAll("_", " ")}${names.length ? `: ${names.join(", ")}` : ""}`;
  if (answer !== undefined) return "Teacher proposed a direct answer.";
  return "Teacher returned a structured proposal.";
}

function proposedNames(
  content: unknown,
  key: "procedures" | "concepts",
): string[] {
  const root = isRecord(content) ? content : {};
  const lesson = recordAt(root, "lesson");
  const values = root[key] ?? lesson?.[key];
  return Array.isArray(values)
    ? values
        .map((item) =>
          isRecord(item) ? stringAt(item, "name", "key", "id") : undefined,
        )
        .filter((name): name is string => Boolean(name))
    : [];
}

function summarizeCheck(value: unknown): JsonRecord | unknown {
  if (!isRecord(value)) return value;
  return compact({
    validator: stringAt(value, "validator"),
    status: stringAt(value, "status"),
    reason: stringAt(value, "reason", "details"),
    evidence: value.evidence,
  });
}

function learningSummary(action: string | undefined): string {
  if (!action || action === "answer-only")
    return "No reusable procedure was learned.";
  if (/reuse|recall/i.test(action)) return "Reused an existing procedure.";
  if (/learn|promot|create/i.test(action))
    return "Learned or promoted reusable knowledge.";
  return `Recorded action: ${action}.`;
}

const INDEX_HTML = `<!doctype html>
<html lang="en"><head><meta charset="utf-8" /><meta name="viewport" content="width=device-width, initial-scale=1" /><title>EKG Inspector</title><style>
:root{color-scheme:dark;font-family:ui-sans-serif,system-ui,sans-serif;background:#101114;color:#eceef2}body{max-width:1180px;margin:0 auto;padding:32px 20px 60px}h1{margin:0 0 8px;letter-spacing:-.03em}h2{margin-top:32px}.muted{color:#9aa0ad}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:12px}.card,table,details{background:#191b21;border:1px solid #2b2e38;border-radius:12px}.card{padding:16px}.value{display:block;font-size:28px;font-weight:700;margin-top:6px}table{width:100%;border-collapse:separate;border-spacing:0;overflow:hidden}th,td{text-align:left;padding:10px 12px;border-bottom:1px solid #2b2e38;vertical-align:top}tr:last-child td{border-bottom:0}th{color:#9aa0ad;font-size:12px;text-transform:uppercase;letter-spacing:.08em}code,pre{color:#b9c6ff;white-space:pre-wrap;overflow-wrap:anywhere}.error{color:#ff9b9b}button{background:#b9c6ff;color:#101114;border:0;border-radius:8px;padding:8px 12px;cursor:pointer}.metric-card{min-height:122px}.metric-head{display:flex;justify-content:space-between;gap:8px;align-items:start}.metric-status{border:1px solid #4b5060;border-radius:999px;color:#c5cad7;font-size:11px;padding:3px 7px;white-space:nowrap}.metric-status.measured{border-color:#6cc58b;color:#9de5b2}.metric-status.partial{border-color:#d9ad61;color:#f2c77f}.metric-detail{display:block;color:#9aa0ad;font-size:13px;line-height:1.4;margin-top:10px}#episode-detail{margin-top:14px}.narrative{padding:18px}.narrative h3{margin:22px 0 8px}.narrative h3:first-child{margin-top:0}.narrative dl{display:grid;grid-template-columns:minmax(130px,220px) 1fr;gap:8px 16px;margin:0}.narrative dt{color:#9aa0ad}.narrative dd{margin:0}details{margin-top:14px;padding:12px}summary{cursor:pointer}.episode-row{cursor:pointer}.episode-row:hover{background:#222633}
</style></head><body>
<h1>EKG Inspector</h1><p class="muted">A local read-only view of the graph, episodes, and flywheel metrics.</p><button id="refresh">Refresh</button><p id="status" class="muted"></p>
<section><h2>Section 38 metric slots</h2><p class="muted">Statuses describe what the current server actually measures; uninstrumented slots are intentionally not scored.</p><div id="metrics" class="grid"></div></section><section><h2>Raw telemetry</h2><div id="telemetry" class="grid"></div></section><section><h2>Knowledge</h2><div id="knowledge"></div></section><section><h2>Recent episodes</h2><p class="muted">Select an episode for the redacted, read-only “What happened?” narrative.</p><div id="episodes"></div><div id="episode-detail"></div></section>
<script>
const $=id=>document.getElementById(id),esc=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])),get=async path=>{const r=await fetch(path);if(!r.ok)throw new Error(await r.text());return r.json()},pretty=value=>value===undefined?'not recorded':esc(typeof value==='string'?value:JSON.stringify(value)),cell=value=>'<code>'+pretty(value)+'</code>',fieldRows=value=>Object.entries(value||{}).filter(([,v])=>v!==undefined&&v!==null&&v!=='').map(([key,v])=>'<dt>'+esc(key.replace(/([A-Z])/g,' $1'))+'</dt><dd>'+pretty(v)+'</dd>').join('');
async function showEpisode(id){$('episode-detail').innerHTML='<p class="muted">Loading episode…</p>';try{const detail=await get('/api/episodes/'+encodeURIComponent(id)),n=detail.narrative,teacher=n.teacher||{},validation=teacher.validation||{};$('episode-detail').innerHTML='<article class="card narrative"><h3>What happened?</h3><dl>'+fieldRows({request:n.request,escalation:n.escalation,learnedOrReused:n.learning&&n.learning.summary,prediction:n.prediction,observation:n.observation,evaluation:n.evaluation,cost:n.cost,abstentionReason:n.abstentionReason})+'</dl><h3>Teacher</h3><dl>'+fieldRows({used:teacher.used?'yes':'no',provider:teacher.provider,model:teacher.model,source:teacher.source,proposalSummary:teacher.proposalSummary,proposal:teacher.proposal,providerError:teacher.providerError,validationStatus:validation.status,validationChecks:validation.checks})+'</dl><h3>Learning</h3><dl>'+fieldRows(n.learning)+'</dl><details><summary>Redacted raw JSON</summary><pre>'+pretty(detail.raw)+'</pre></details></article>'}catch(error){$('episode-detail').innerHTML='<p class="error">'+esc(error.message||error)+'</p>'}}
async function refresh(){$('status').textContent='Refreshing…';try{const[metrics,concepts,procedures,episodes]=await Promise.all([get('/api/metrics'),get('/api/concepts'),get('/api/procedures'),get('/api/episodes')]),i=metrics.intuition,p=metrics.phase6||{},groundedRatio=i.supervisionTasks>0?(i.groundedTasks/i.supervisionTasks*100).toFixed(1)+'%':'No tasks yet',skills=p.managedSkillRecordsExamined??0,slots=[['1. Compounding','not-instrumented','Not instrumented','Needs cost of the Nth skill over a comparable task sequence.'],['2. Transfer','partial','Partial evidence','Persisted transfer wins: '+(p.transferEligibleSkillVerdicts??0)+' among '+skills+' examined skill records. This is promotion-gate evidence, not held-out task-family coverage.'],['3. Per-domain weaning','partial','Partial evidence','Teacher-request episodes: '+(p.teacherInteractionEpisodes??0)+'; successful teacher-free episodes: '+(p.teacherFreeSuccesses??0)+'; teacher-assisted successes: '+(p.teacherAssistedSuccesses??0)+'. No domains or comparable time cohorts are recorded.'],['4. Trace compression','not-instrumented','Not instrumented','Needs repeated task-family traces over time.'],['5. Rung distribution','measured','Measured',metrics.rungDistribution.length?'Current episode distribution is available.':'No episode rung data yet.'],['6. No regression','partial','Partial evidence','Verified baselines: '+metrics.verifiedAnswerCount+'; preserved replay verdicts: '+(p.replayPreservedSkillVerdicts??0)+'; regression verdicts: '+(p.replayRegressions??0)+'. No fresh full-suite replay is implied.'],['7. Attribution accuracy','not-instrumented','Not instrumented','Needs injected-fault outcomes and credit comparisons.'],['8. Attribution cost','not-instrumented','Not instrumented','Needs attribution and total-cost traces together.'],['9. Teacher ablation','not-instrumented','Not instrumented','Needs task-history replay with the teacher disconnected.'],['10. Grounding drift','partial','Partial signal','Grounded supervision share: '+groundedRatio+' (not a belief-level measure).'],['11. Abstraction survival','partial','Partial evidence','Recorded post-promotion success: '+(p.postPromotionSkillSuccesses??0)+' of '+(p.postPromotionSkillUses??0)+' uses across '+(p.currentlyPromotedSkills??0)+' currently promoted skills. Zero is not evidence of non-survival.'],['12. Calibration','not-instrumented','Not instrumented','Needs confidence values paired with observed correctness.']];$('metrics').innerHTML=slots.map(([label,statusClass,status,detail])=>'<div class="card metric-card"><div class="metric-head"><span class="muted">'+label+'</span><span class="metric-status '+statusClass+'">'+status+'</span></div><span class="metric-detail">'+esc(detail)+'</span></div>').join('');$('telemetry').innerHTML=[['Episodes',metrics.episodeCount],['Teacher requests',p.teacherInteractionEpisodes??0],['Verified baselines',metrics.verifiedAnswerCount],['Preserved replay verdicts',p.replayPreservedSkillVerdicts??0],['Transfer wins',p.transferEligibleSkillVerdicts??0],['Post-promotion successes',p.postPromotionSkillSuccesses??0],['Indexed docs',i.indexedDocuments],['Recall queries',i.retrievalQueries],['Candidates examined',i.candidatesExamined],['Ranking examples',i.rankingExamples],['Grounded tasks',i.groundedTasks]].map(([label,value])=>'<div class="card"><span class="muted">'+label+'</span><span class="value">'+esc(value)+'</span></div>').join('');$('knowledge').innerHTML='<table><thead><tr><th>Type</th><th>Name</th><th>Id</th></tr></thead><tbody>'+concepts.map(c=>'<tr><td>Concept</td><td>'+esc(c.name)+'</td><td>'+cell(c.id)+'</td></tr>').concat(procedures.map(p=>'<tr><td>Procedure</td><td>'+esc(p.name)+'</td><td>'+cell(p.id)+'</td></tr>')).join('')+'</tbody></table>';$('episodes').innerHTML='<table><thead><tr><th>Situation</th><th>Disposition</th><th>Rung</th><th>Result</th></tr></thead><tbody>'+episodes.map(e=>'<tr class="episode-row" data-episode-id="'+esc(e.id)+'"><td>'+esc(e.situation)+'</td><td>'+cell(e.evaluation?.success===true?'success':e.evaluation?.success===false?'failure':'unverified')+'</td><td>'+cell(e.cost?.rungReached??e.cost?.rung_reached)+'</td><td>'+cell(e.observedResult??e.observed_result)+'</td></tr>').join('')+'</tbody></table>';document.querySelectorAll('[data-episode-id]').forEach(row=>row.addEventListener('click',()=>showEpisode(row.dataset.episodeId)));$('status').textContent='Updated '+new Date().toLocaleTimeString()}catch(error){$('status').innerHTML='<span class="error">'+esc(error.message||error)+'</span>'}}
$('refresh').addEventListener('click',refresh);refresh();
</script></body></html>`;

function start(): void {
  const port = Number.parseInt(process.env.EKG_INSPECTOR_PORT ?? "4317", 10);
  const transport = StdioTransport.spawn(
    process.env.EKG_SERVER ??
      fileURLToPath(
        new URL("../../../target/debug/ekg-server", import.meta.url),
      ),
  );
  const client = new EkgClient(transport, {
    adminToken: process.env.EKG_ADMIN_TOKEN,
  });
  const server = createInspectorServer(client);
  server.listen(port, "127.0.0.1", () =>
    console.log(`EKG Inspector listening at http://127.0.0.1:${port}`),
  );
  const close = () => {
    client.close();
    server.close();
  };
  process.once("SIGINT", close);
  process.once("SIGTERM", close);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1])
  start();
