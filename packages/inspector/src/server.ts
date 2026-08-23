import { createServer, type Server, type ServerResponse } from "node:http";
import { fileURLToPath } from "node:url";

import { EkgClient, StdioTransport, type JsonValue } from "@ekg/sdk";

type JsonRecord = Record<string, unknown>;

interface InspectorClient {
  metricsSnapshot(): Promise<unknown>;
  listConcepts(): Promise<unknown>;
  listProcedures(): Promise<unknown>;
  listEpisodes(filter: { limit: number }): Promise<unknown>;
  getEpisode(episodeId: string): Promise<unknown>;
  /** Optional until the server exposes a relationship collection RPC. */
  listRelationships?: () => Promise<unknown>;
  getProcedure?: (procedureId: string) => Promise<unknown>;
  listProcedureVersions?: (procedureId: string) => Promise<unknown>;
  getProcedureVersion?: (
    procedureId: string,
    version: number,
  ) => Promise<unknown>;
  listContradictions?: () => Promise<unknown>;
  getContradiction?: (contradictionId: number) => Promise<unknown>;
  replayEpisode?: (
    episodeId: string,
    substitutions: Record<string, JsonValue>,
  ) => Promise<unknown>;
}

export interface EpisodeDetail {
  narrative: JsonRecord;
  raw: unknown;
}

export interface KnowledgeGraphProjection {
  nodes: JsonRecord[];
  edges: JsonRecord[];
  bounded: boolean;
  maxNodes: number;
  maxEdges: number;
}

export interface ProcedureDetail {
  procedure: unknown;
  versions: unknown[];
  historyAvailable: boolean;
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
  const proposal = teacher ? recordAt(teacher, "proposal") : undefined;
  const provenance = teacher
    ? (recordAt(teacher, "provenance") ??
      recordAt(proposal, "provenance") ??
      teacher)
    : undefined;
  const validation = teacher
    ? (recordAt(proposal, "validation") ?? recordAt(teacher, "validation"))
    : undefined;
  const evaluation = recordAt(episode, "evaluation");
  const cost = recordAt(episode, "cost") ?? {};
  const action = stringAt(episode, "action");
  const proposedContent = proposal?.content ?? teacher?.content;
  const proposalKind = stringAt(
    isRecord(proposedContent) ? proposedContent : undefined,
    "proposalKind",
    "proposal_kind",
  );
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
        proposalKind,
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
        answerSource:
          action === "teacher-observation:provisional"
            ? "teacher-provided external observation (unverified)"
            : undefined,
        reusableKnowledge:
          action === "teacher-observation:provisional" ? false : undefined,
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

/** Build a small, redacted graph projection for the browser. */
export function knowledgeGraph(
  conceptsValue: unknown,
  proceduresValue: unknown,
  relationshipsValue: unknown = [],
  maxNodes = 200,
  maxEdges = 300,
): KnowledgeGraphProjection {
  const concepts = Array.isArray(conceptsValue)
    ? conceptsValue
        .filter(isRecord)
        .map((item) => redactSensitive(item))
        .filter(isRecord)
    : [];
  const procedures = Array.isArray(proceduresValue)
    ? proceduresValue
        .filter(isRecord)
        .map((item) => redactSensitive(item))
        .filter(isRecord)
    : [];
  const relationships = Array.isArray(relationshipsValue)
    ? relationshipsValue
        .filter(isRecord)
        .map((item) => redactSensitive(item))
        .filter(isRecord)
    : [];
  const nodes: JsonRecord[] = [];
  const known = new Set<string>();
  const addNode = (node: JsonRecord): void => {
    const id = stringAt(node, "id", "conceptId", "procedureId");
    if (!id || known.has(id) || nodes.length >= maxNodes) return;
    known.add(id);
    nodes.push({
      id,
      kind: stringAt(node, "kind") ?? (node.params ? "procedure" : "concept"),
      name: stringAt(node, "name") ?? id,
      lifecycle: valueAt(node, "lifecycle"),
      version: valueAt(node, "version"),
    });
  };
  for (const concept of concepts) addNode(concept);
  for (const procedure of procedures) addNode(procedure);

  const edges: JsonRecord[] = [];
  const addEdge = (
    source: unknown,
    target: unknown,
    kind: string,
    metadata?: JsonRecord,
  ): void => {
    if (edges.length >= maxEdges) return;
    const sourceId =
      typeof source === "string"
        ? source
        : stringAt(source as JsonRecord, "id");
    const targetId =
      typeof target === "string"
        ? target
        : stringAt(target as JsonRecord, "id");
    if (!sourceId || !targetId || !known.has(sourceId) || !known.has(targetId))
      return;
    edges.push(
      compact({ source: sourceId, target: targetId, kind, ...metadata }),
    );
  };
  for (const relationship of relationships) {
    addEdge(
      valueAt(relationship, "source", "sourceId", "source_id"),
      valueAt(relationship, "target", "targetId", "target_id"),
      stringAt(relationship, "kind", "type") ?? "related",
      compact({
        id: stringAt(relationship, "id"),
        strength: valueAt(relationship, "strength"),
      }),
    );
  }
  for (const procedure of procedures) {
    const procedureId = stringAt(
      procedure,
      "id",
      "procedureId",
      "procedure_id",
    );
    const concept = valueAt(procedure, "concept", "conceptId", "concept_id");
    addEdge(procedureId, concept, "implements");
    const dependencies = valueAt(procedure, "dependencies", "calls");
    if (Array.isArray(dependencies)) {
      for (const dependency of dependencies) {
        const dependencyId =
          typeof dependency === "string"
            ? dependency
            : stringAt(
                dependency as JsonRecord,
                "id",
                "procedureId",
                "procedure_id",
              );
        addEdge(procedureId, dependencyId, "depends_on");
      }
    }
  }
  return {
    nodes,
    edges,
    bounded:
      nodes.length < concepts.length + procedures.length ||
      edges.length >= maxEdges,
    maxNodes,
    maxEdges,
  };
}

export function procedureDetail(
  procedureValue: unknown,
  versionsValue?: unknown,
): ProcedureDetail {
  const procedure = redactSensitive(procedureValue);
  const versions = Array.isArray(versionsValue)
    ? versionsValue.map((item) => redactSensitive(item))
    : [];
  return {
    procedure,
    versions,
    historyAvailable: versionsValue !== undefined,
  };
}

export function contradictionDetail(value: unknown): JsonRecord {
  const redacted = redactSensitive(value);
  return isRecord(redacted) ? redacted : { value: redacted };
}

function filteredEpisodes(value: unknown, query: string | null): unknown {
  if (!query?.trim() || !Array.isArray(value)) return redactSensitive(value);
  const needle = query.trim().toLowerCase();
  return redactSensitive(
    value.filter((episode) => {
      if (!isRecord(episode)) return false;
      return ["id", "situation", "action"]
        .map((key) => episode[key])
        .some((candidate) =>
          String(candidate ?? "")
            .toLowerCase()
            .includes(needle),
        );
    }),
  );
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
      if (url.pathname === "/api/knowledge") {
        const [concepts, procedures, relationships] = await Promise.all([
          client.listConcepts(),
          client.listProcedures(),
          client.listRelationships?.() ?? Promise.resolve([]),
        ]);
        return json(
          response,
          knowledgeGraph(concepts, procedures, relationships),
        );
      }
      if (url.pathname === "/api/relationships")
        return json(
          response,
          redactSensitive((await client.listRelationships?.()) ?? []),
        );
      if (url.pathname === "/api/contradictions")
        return json(
          response,
          redactSensitive((await client.listContradictions?.()) ?? []),
        );
      const procedureId = procedureIdFromPath(url.pathname);
      if (procedureId) {
        if (!client.getProcedure)
          return json(
            response,
            { error: "procedure detail is unavailable" },
            501,
          );
        const procedure = await client.getProcedure(procedureId);
        const versions = client.listProcedureVersions
          ? await client.listProcedureVersions(procedureId)
          : undefined;
        return json(response, procedureDetail(procedure, versions));
      }
      const contradictionId = contradictionIdFromPath(url.pathname);
      if (contradictionId !== undefined) {
        if (!client.getContradiction)
          return json(
            response,
            { error: "contradiction detail is unavailable" },
            501,
          );
        return json(
          response,
          contradictionDetail(await client.getContradiction(contradictionId)),
        );
      }
      if (url.pathname === "/api/episodes") {
        const episodes = await client.listEpisodes({ limit: 200 });
        return json(
          response,
          filteredEpisodes(episodes, url.searchParams.get("q")),
        );
      }
      const replayId = replayIdFromPath(url.pathname);
      if (replayId) {
        if (!client.replayEpisode)
          return json(
            response,
            { error: "episode replay is unavailable" },
            501,
          );
        const substitutions = parseJsonRecord(
          url.searchParams.get("substitutions"),
        );
        return json(
          response,
          compact({
            readOnly: true,
            episodeId: replayId,
            substitutions: redactSensitive(substitutions),
            result: redactSensitive(
              await client.replayEpisode(replayId, substitutions),
            ),
          }),
        );
      }
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

function replayIdFromPath(pathname: string): string | undefined {
  const match = /^\/api\/episodes\/([^/]+)\/replay$/.exec(pathname);
  return match ? decodeURIComponent(match[1]!) : undefined;
}

function procedureIdFromPath(pathname: string): string | undefined {
  const match = /^\/api\/procedures\/([^/]+)$/.exec(pathname);
  return match ? decodeURIComponent(match[1]!) : undefined;
}

function contradictionIdFromPath(pathname: string): number | undefined {
  const match = /^\/api\/contradictions\/([^/]+)$/.exec(pathname);
  if (!match) return undefined;
  const id = Number.parseInt(decodeURIComponent(match[1]!), 10);
  return Number.isSafeInteger(id) ? id : undefined;
}

function parseJsonRecord(value: string | null): Record<string, JsonValue> {
  if (!value) return {};
  try {
    const parsed: unknown = JSON.parse(value);
    return isRecord(parsed) ? (parsed as Record<string, JsonValue>) : {};
  } catch {
    return {};
  }
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
  // Keep the human-facing Section 38 cards sourced from the same immutable
  // probe store as the RPC snapshot. The legacy flywheel counters remain
  // visible elsewhere, but cannot silently stand in for held-out evidence.
  return INDEX_HTML.replace("</body>", `${SECTION38_RUNTIME_PATCH}</body>`);
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
  if (kind) {
    const suffix = names.length
      ? `: ${names.join(", ")}`
      : answer !== undefined
        ? `: ${typeof answer === "string" ? answer : JSON.stringify(answer)}`
        : "";
    return `${kind.replaceAll("_", " ")}${suffix}`;
  }
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
  if (action === "teacher-observation:provisional")
    return "Teacher supplied a provisional external answer; no reusable lesson or procedure was proposed or admitted.";
  if (/reuse|recall/i.test(action)) return "Reused an existing procedure.";
  if (/learn|promot|create/i.test(action))
    return "Learned or promoted reusable knowledge.";
  return `Recorded action: ${action}.`;
}

const SECTION38_RUNTIME_PATCH = `<script>
async function renderSection38Telemetry(){
  try{
    const snapshot=await get('/api/metrics'),telemetry=snapshot.section38||{},slots=telemetry.metrics||[];
    if(!slots.length)return;
    $('metrics').innerHTML=slots.map(metric=>{
      const measured=metric.status==='measured',status=measured?'Measured':'Insufficient evidence',value=metric.value===null||metric.value===undefined?'':(' Value: '+Number(metric.value).toFixed(3)+'.');
      return '<div class="card metric-card"><div class="metric-head"><span class="muted">'+esc(metric.slot+'. '+metric.name)+'</span><span class="metric-status '+(measured?'measured':'partial')+'">'+esc(status)+'</span></div><span class="metric-detail">n='+esc(metric.sampleSize)+'. '+esc(metric.detail)+esc(value)+'</span></div>';
    }).join('');
    const tele=$('telemetry');
    if(tele){tele.querySelectorAll('.section38-telemetry').forEach(node=>node.remove());tele.insertAdjacentHTML('afterbegin',[['Probe measurements',telemetry.measurements],['Probe failures',telemetry.failures],['Abstentions',telemetry.abstentions],['Clarifications',telemetry.clarifications],['Rejected teacher-off claims',telemetry.teacherOffViolationsRejected],['Rejected undeclared repeats',telemetry.duplicateMeasurementsRejected]].map(pair=>'<div class="card section38-telemetry"><span class="muted">'+esc(pair[0])+'</span><span class="value">'+esc(pair[1])+'</span></div>').join(''))}
  }catch(error){/* The normal inspector refresh reports transport failures. */}
}
setTimeout(renderSection38Telemetry,50);
document.getElementById('refresh')?.addEventListener('click',()=>setTimeout(renderSection38Telemetry,100));
</script>`;

const LEGACY_INDEX_HTML = `<!doctype html>
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

const INDEX_HTML = `<!doctype html>
<html lang="en"><head><meta charset="utf-8" /><meta name="viewport" content="width=device-width, initial-scale=1" /><title>EKG Inspector</title><style>
:root{color-scheme:dark;font-family:ui-sans-serif,system-ui,sans-serif;background:#101114;color:#eceef2}body{max-width:1240px;margin:0 auto;padding:32px 20px 60px}h1{margin:0 0 8px;letter-spacing:-.03em}h2{margin-top:32px}.muted{color:#9aa0ad}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:12px}.card,table,details{background:#191b21;border:1px solid #2b2e38;border-radius:12px}.card{padding:16px}.value{display:block;font-size:28px;font-weight:700;margin-top:6px}table{width:100%;border-collapse:separate;border-spacing:0;overflow:hidden}th,td{text-align:left;padding:10px 12px;border-bottom:1px solid #2b2e38;vertical-align:top}tr:last-child td{border-bottom:0}th{color:#9aa0ad;font-size:12px;text-transform:uppercase;letter-spacing:.08em}code,pre{color:#b9c6ff;white-space:pre-wrap;overflow-wrap:anywhere}.error{color:#ff9b9b}button{background:#b9c6ff;color:#101114;border:0;border-radius:8px;padding:8px 12px;cursor:pointer}button.secondary{background:#2b2e38;color:#eceef2}.controls{display:flex;flex-wrap:wrap;gap:8px;align-items:center}.controls input,.controls select{background:#101114;color:#eceef2;border:1px solid #4b5060;border-radius:8px;padding:8px}.metric-card{min-height:122px}.metric-head{display:flex;justify-content:space-between;gap:8px;align-items:start}.metric-status{border:1px solid #4b5060;border-radius:999px;color:#c5cad7;font-size:11px;padding:3px 7px;white-space:nowrap}.metric-status.measured{border-color:#6cc58b;color:#9de5b2}.metric-status.partial{border-color:#d9ad61;color:#f2c77f}.metric-detail{display:block;color:#9aa0ad;font-size:13px;line-height:1.4;margin-top:10px}.panel{margin-top:14px}.narrative{padding:18px}.narrative h3{margin:22px 0 8px}.narrative h3:first-child{margin-top:0}.narrative dl{display:grid;grid-template-columns:minmax(130px,220px) 1fr;gap:8px 16px;margin:0}.narrative dt{color:#9aa0ad}.narrative dd{margin:0}details{margin-top:14px;padding:12px}summary{cursor:pointer}.selectable{cursor:pointer}.selectable:hover{background:#222633}.graph{display:grid;grid-template-columns:minmax(240px,1fr) minmax(320px,1.5fr);gap:14px}.scroll{overflow:auto;max-height:420px}@media(max-width:720px){.graph{grid-template-columns:1fr}.narrative dl{grid-template-columns:1fr;gap:3px 0}}
</style></head><body>
<h1>EKG Inspector</h1><p class="muted">A local, read-only view of knowledge, evidence, and the learning flywheel. Nothing here mutates the graph.</p><div class="controls"><button id="refresh">Refresh</button><span id="status" class="muted"></span></div>
<section><h2>Section 38 metric slots</h2><p class="muted">Statuses describe what the current server actually measures; uninstrumented slots are intentionally not scored.</p><div id="metrics" class="grid"></div></section>
<section><h2>Raw telemetry</h2><div id="telemetry" class="grid"></div></section>
<section><h2>Knowledge graph</h2><p class="muted">The graph is bounded for safety and legibility. Relationships are shown when the server exposes the read-only relationship collection.</p><div class="graph"><div id="knowledge"></div><div id="graph-edges" class="card scroll"></div></div><div id="procedure-detail" class="panel"></div></section>
<section><h2>Contradictions</h2><p class="muted">Held disagreements stay visible with their claims, supporting episodes, scopes, and any later refinement.</p><div id="contradictions"></div><div id="contradiction-detail" class="panel"></div></section>
<section><h2>Episodes</h2><p class="muted">Search or filter episodes, then open one for provenance. Replay is explicitly read-only and remains redacted.</p><div class="controls"><input id="episode-search" type="search" placeholder="Search request or action" /><select id="episode-outcome"><option value="">Any outcome</option><option value="success">Success</option><option value="failure">Failure</option></select><button id="episode-filter" class="secondary">Apply</button></div><div id="episodes" class="panel"></div><div id="episode-detail" class="panel"></div></section>
<script>
const $=id=>document.getElementById(id),esc=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])),get=async path=>{const r=await fetch(path);if(!r.ok)throw new Error(await r.text());return r.json()},pretty=value=>value===undefined?'not recorded':esc(typeof value==='string'?value:JSON.stringify(value)),cell=value=>'<code>'+pretty(value)+'</code>',fieldRows=value=>Object.entries(value||{}).filter(([,v])=>v!==undefined&&v!==null&&v!=='').map(([key,v])=>'<dt>'+esc(key.replace(/([A-Z])/g,' $1'))+'</dt><dd>'+pretty(v)+'</dd>').join('');
async function showEpisode(id){$('episode-detail').innerHTML='<p class="muted">Loading episode…</p>';try{const detail=await get('/api/episodes/'+encodeURIComponent(id)),n=detail.narrative,teacher=n.teacher||{},validation=teacher.validation||{};$('episode-detail').innerHTML='<article class="card narrative"><h3>What happened?</h3><dl>'+fieldRows({request:n.request,escalation:n.escalation,learnedOrReused:n.learning&&n.learning.summary,prediction:n.prediction,observation:n.observation,evaluation:n.evaluation,cost:n.cost,abstentionReason:n.abstentionReason})+'</dl><h3>Teacher and provenance</h3><dl>'+fieldRows({used:teacher.used?'yes':'no',provider:teacher.provider,model:teacher.model,source:teacher.source,proposalKind:teacher.proposalKind,proposalSummary:teacher.proposalSummary,proposal:teacher.proposal,providerError:teacher.providerError,validationStatus:validation.status,validationChecks:validation.checks})+'</dl><h3>Learning</h3><dl>'+fieldRows(n.learning)+'</dl><button class="secondary" data-replay="'+esc(id)+'">Replay read-only</button><div id="replay-result"></div><details><summary>Redacted raw JSON</summary><pre>'+pretty(detail.raw)+'</pre></details></article>';$('[data-replay]').addEventListener('click',async()=>{const output=$('replay-result');output.innerHTML='<p class="muted">Replaying…</p>';try{const replay=await get('/api/episodes/'+encodeURIComponent(id)+'/replay');output.innerHTML='<p>Replay result (read-only):</p><pre>'+pretty(replay)+'</pre>'}catch(error){output.innerHTML='<p class="error">'+esc(error.message||error)+'</p>'}})}catch(error){$('episode-detail').innerHTML='<p class="error">'+esc(error.message||error)+'</p>'}}
async function showProcedure(id){$('procedure-detail').innerHTML='<p class="muted">Loading procedure…</p>';try{const detail=await get('/api/procedures/'+encodeURIComponent(id)),p=detail.procedure||{};$('procedure-detail').innerHTML='<article class="card narrative"><h3>Procedure: '+esc(p.name||p.id)+'</h3><dl>'+fieldRows({id:p.id,version:p.version,lifecycle:p.lifecycle,parameters:p.params,contract:p.contract,tests:p.testCases||p.test_cases,concept:p.concept})+'</dl><h3>Version history</h3><p class="muted">'+(detail.historyAvailable?(detail.versions.length+' recorded version(s).'): 'Version history is not exposed by this server.')+'</p><pre>'+pretty(detail.versions)+'</pre></article>'}catch(error){$('procedure-detail').innerHTML='<p class="error">'+esc(error.message||error)+'</p>'}}
async function showContradiction(id){$('contradiction-detail').innerHTML='<p class="muted">Loading contradiction…</p>';try{const value=await get('/api/contradictions/'+encodeURIComponent(id));$('contradiction-detail').innerHTML='<article class="card narrative"><h3>Contradiction '+esc(id)+'</h3><pre>'+pretty(value)+'</pre></article>'}catch(error){$('contradiction-detail').innerHTML='<p class="error">'+esc(error.message||error)+'</p>'}}
function renderKnowledge(graph,procedures){const names=new Map((graph.nodes||[]).map(n=>[n.id,n.name]));$('knowledge').innerHTML='<div class="card scroll"><strong>'+esc((graph.nodes||[]).length)+' nodes</strong><table><thead><tr><th>Type</th><th>Name</th><th>Version</th></tr></thead><tbody>'+((graph.nodes||[]).map(n=>'<tr><td>'+esc(n.kind)+'</td><td>'+esc(n.name)+'</td><td>'+cell(n.version)+'</td></tr>').join('')||'<tr><td colspan="3" class="muted">No knowledge yet.</td></tr>')+'</tbody></table></div>';$("graph-edges").innerHTML='<strong>Bounded edges</strong><p class="muted">'+esc((graph.edges||[]).length)+' edge(s); max '+esc(graph.maxEdges)+'</p>'+((graph.edges||[]).map(e=>'<p><code>'+esc(names.get(e.source)||e.source)+'</code> → <code>'+esc(names.get(e.target)||e.target)+'</code> <span class="muted">('+esc(e.kind)+')</span></p>').join('')||'<p class="muted">No relationships available.</p>');const procedureRows=(procedures||[]).map(p=>'<tr class="selectable" data-procedure-id="'+esc(p.id)+'"><td>Procedure</td><td>'+esc(p.name)+'</td><td>'+cell(p.version)+'</td></tr>').join('');$('knowledge').innerHTML+='<div class="card"><h3>Procedures</h3><table><thead><tr><th>Type</th><th>Name</th><th>Version</th></tr></thead><tbody>'+procedureRows+'</tbody></table></div>';document.querySelectorAll('[data-procedure-id]').forEach(row=>row.addEventListener('click',()=>showProcedure(row.dataset.procedureId)))}
async function refreshEpisodes(){const q=$('episode-search').value.trim(),outcome=$('episode-outcome').value,path='/api/episodes'+(q?'?q='+encodeURIComponent(q):'');const episodes=await get(path);const rows=(episodes||[]).map(e=>{const status=e.evaluation?.success===true?'success':e.evaluation?.success===false?'failure':'unverified';return !outcome||status===outcome?'<tr class="selectable" data-episode-id="'+esc(e.id)+'"><td>'+esc(e.situation)+'</td><td>'+cell(status)+'</td><td>'+cell(e.cost?.rungReached??e.cost?.rung_reached)+'</td><td>'+cell(e.observedResult??e.observed_result)+'</td></tr>':''}).join('');$('episodes').innerHTML='<table><thead><tr><th>Request</th><th>Disposition</th><th>Rung</th><th>Result</th></tr></thead><tbody>'+ (rows||'<tr><td colspan="4" class="muted">No matching episodes.</td></tr>')+'</tbody></table>';document.querySelectorAll('[data-episode-id]').forEach(row=>row.addEventListener('click',()=>showEpisode(row.dataset.episodeId)))}
async function refresh(){$('status').textContent='Refreshing…';try{const[metrics,graph,procedures,contradictions]=await Promise.all([get('/api/metrics'),get('/api/knowledge'),get('/api/procedures'),get('/api/contradictions')]),i=metrics.intuition,p=metrics.phase6||{},groundedRatio=i.supervisionTasks>0?(i.groundedTasks/i.supervisionTasks*100).toFixed(1)+'%':'No tasks yet',skills=p.managedSkillRecordsExamined??0,slots=[['1. Compounding','not-instrumented','Not instrumented','Needs cost of the Nth skill over a comparable task sequence.'],['2. Transfer','partial','Partial evidence','Persisted transfer wins: '+(p.transferEligibleSkillVerdicts??0)+' among '+skills+' examined skill records. This is promotion-gate evidence, not held-out task-family coverage.'],['3. Per-domain weaning','partial','Partial evidence','Teacher-request episodes: '+(p.teacherInteractionEpisodes??0)+'; successful teacher-free episodes: '+(p.teacherFreeSuccesses??0)+'; teacher-assisted successes: '+(p.teacherAssistedSuccesses??0)+'. No domains or comparable time cohorts are recorded.'],['4. Trace compression','not-instrumented','Not instrumented','Needs repeated task-family traces over time.'],['5. Rung distribution','measured','Measured',metrics.rungDistribution.length?'Current episode distribution is available.':'No episode rung data yet.'],['6. No regression','partial','Partial evidence','Verified baselines: '+metrics.verifiedAnswerCount+'; preserved replay verdicts: '+(p.replayPreservedSkillVerdicts??0)+'; regression verdicts: '+(p.replayRegressions??0)+'. No fresh full-suite replay is implied.'],['7. Attribution accuracy','not-instrumented','Not instrumented','Needs injected-fault outcomes and credit comparisons.'],['8. Attribution cost','not-instrumented','Not instrumented','Needs attribution and total-cost traces together.'],['9. Teacher ablation','not-instrumented','Not instrumented','Needs task-history replay with the teacher disconnected.'],['10. Grounding drift','partial','Partial signal','Grounded supervision share: '+groundedRatio+' (not a belief-level measure).'],['11. Abstraction survival','partial','Partial evidence','Recorded post-promotion success: '+(p.postPromotionSkillSuccesses??0)+' of '+(p.postPromotionSkillUses??0)+' uses across '+(p.currentlyPromotedSkills??0)+' currently promoted skills. Zero is not evidence of non-survival.'],['12. Calibration','not-instrumented','Not instrumented','Needs confidence values paired with observed correctness.']];$('metrics').innerHTML=slots.map(([label,statusClass,status,detail])=>'<div class="card metric-card"><div class="metric-head"><span class="muted">'+label+'</span><span class="metric-status '+statusClass+'">'+status+'</span></div><span class="metric-detail">'+esc(detail)+'</span></div>').join('');$('telemetry').innerHTML=[['Episodes',metrics.episodeCount],['Teacher requests',p.teacherInteractionEpisodes??0],['Verified baselines',metrics.verifiedAnswerCount],['Preserved replay verdicts',p.replayPreservedSkillVerdicts??0],['Transfer wins',p.transferEligibleSkillVerdicts??0],['Post-promotion successes',p.postPromotionSkillSuccesses??0],['Indexed docs',i.indexedDocuments],['Recall queries',i.retrievalQueries],['Candidates examined',i.candidatesExamined],['Ranking examples',i.rankingExamples],['Grounded tasks',i.groundedTasks]].map(([label,value])=>'<div class="card"><span class="muted">'+label+'</span><span class="value">'+esc(value)+'</span></div>').join('');renderKnowledge(graph,procedures);$('contradictions').innerHTML='<table><thead><tr><th>Id</th><th>Status</th><th>Left claim</th><th>Right claim</th></tr></thead><tbody>'+((contradictions||[]).map(c=>'<tr class="selectable" data-contradiction-id="'+esc(c.id)+'"><td>'+cell(c.id)+'</td><td>'+esc(c.status)+'</td><td>'+esc(c.left?.statement)+'</td><td>'+esc(c.right?.statement)+'</td></tr>').join('')||'<tr><td colspan="4" class="muted">No held contradictions.</td></tr>')+'</tbody></table>';document.querySelectorAll('[data-contradiction-id]').forEach(row=>row.addEventListener('click',()=>showContradiction(row.dataset.contradictionId)));await refreshEpisodes();$('status').textContent='Updated '+new Date().toLocaleTimeString()}catch(error){$('status').innerHTML='<span class="error">'+esc(error.message||error)+'</span>'}}
$('refresh').addEventListener('click',refresh);$('episode-filter').addEventListener('click',refreshEpisodes);refresh();
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
