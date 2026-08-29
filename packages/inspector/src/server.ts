import { createServer, type Server, type ServerResponse } from "node:http";
import { fileURLToPath } from "node:url";

import { SpoonClient, StdioTransport, type JsonValue } from "@spoon/sdk";

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

  const selectionReason = teacher
    ? recordAt(teacher, "selectionReason")
    : undefined;
  const languageInterpreter = findLanguageInterpreter(teacher);
  const teacherUsed = teacherWasUsed(teacher, languageInterpreter);
  const reasoningTrace = episode.reasoning_trace ?? episode.reasoningTrace;
  const traceSteps = Array.isArray(
    isRecord(reasoningTrace)
      ? (reasoningTrace as Record<string, unknown>).steps
      : undefined,
  )
    ? ((reasoningTrace as Record<string, unknown>).steps as unknown[])
    : Array.isArray(reasoningTrace)
      ? (reasoningTrace as unknown[])
      : undefined;

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
      decisionPath: traceSteps
        ? traceSteps.filter(isRecord).map((step) =>
            compact({
              description: stringAt(step, "description"),
              rung: stringAt(step, "rung"),
              status:
                step.status === "Succeeded" || isRecord(step.status)
                  ? typeof step.status === "string"
                    ? step.status
                    : "Failed"
                  : step.status,
              error: isRecord(step.status)
                ? stringAt(step.status as Record<string, unknown>, "error")
                : undefined,
              procedure: stringAt(step, "procedure_used", "procedureUsed"),
            }),
          )
        : undefined,
      selectionReason: selectionReason
        ? compact({
            path: stringAt(selectionReason, "path"),
            summary: stringAt(selectionReason, "summary"),
            concept: stringAt(selectionReason, "concept"),
            procedure: stringAt(selectionReason, "procedure"),
            version: valueAt(selectionReason, "version"),
            source: stringAt(selectionReason, "source"),
            disposition: stringAt(selectionReason, "disposition"),
            chain: selectionReason.chain,
            slotBindings:
              selectionReason.slotBindings ?? selectionReason.inputBindings,
          })
        : undefined,
      interpreter: interpreterNarrative(languageInterpreter),
      teacher: compact({
        used: teacherUsed,
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

/**
 * Teacher handoff records the interpreter under `priorFailure` so the chain
 * stays durable. Walk that wrapper or an interpreter-only interaction is
 * misreported as missing.
 */
function findLanguageInterpreter(
  value: unknown,
  depth = 0,
): JsonRecord | undefined {
  if (!isRecord(value) || depth > 4) return undefined;
  if (isRecord(value.languageInterpreter)) return value.languageInterpreter;
  for (const key of ["priorFailure", "rejectedTeacherInteraction"]) {
    const found = findLanguageInterpreter(value[key], depth + 1);
    if (found !== undefined) return found;
  }
  return undefined;
}

function teacherWasUsed(
  teacher: JsonRecord | undefined,
  languageInterpreter: JsonRecord | undefined,
): boolean {
  if (!teacher) return false;
  if (recordAt(teacher, "proposal") || isRecord(teacher.content)) return true;
  return teacher.request !== undefined && languageInterpreter === undefined;
}

function interpreterNarrative(
  languageInterpreter: JsonRecord | undefined,
): JsonRecord | undefined {
  if (!languageInterpreter) return undefined;
  const provenance = recordAt(languageInterpreter, "provenance");
  const frames = recordAt(languageInterpreter, "frames");
  const request = recordAt(languageInterpreter, "request");
  const requestContext = request ? (recordAt(request, "context") ?? {}) : {};
  const rejected = recordAt(languageInterpreter, "rejectedProposal");
  const rejectedContent = rejected ? recordAt(rejected, "content") : undefined;
  const rawContent = rejected
    ? recordAt(rejected, "rawContent", "raw_content")
    : undefined;
  const selected = frames ? valueAt(frames, "selected") : undefined;
  const frameCandidates = Array.isArray(frames?.candidates)
    ? (frames.candidates as unknown[])
    : [];
  const requestCandidates = requestContext.candidates;
  const priorTurns = requestContext.priorTurns ?? requestContext.prior_turns;

  return compact({
    used: true,
    source: stringAt(languageInterpreter, "source"),
    status: stringAt(languageInterpreter, "status"),
    provider: stringAt(provenance, "provider"),
    model: stringAt(provenance, "model"),
    disposition: stringAt(frames, "disposition"),
    selected: typeof selected === "number" ? selected : undefined,
    candidateCount: Array.isArray(requestCandidates)
      ? requestCandidates.length
      : undefined,
    priorTurnCount: Array.isArray(priorTurns) ? priorTurns.length : undefined,
    providerError: stringAt(
      languageInterpreter,
      "providerError",
      "provider_error",
    ),
    rejection: stringAt(languageInterpreter, "rejection"),
    candidates: frameCandidates.length
      ? frameCandidates.filter(isRecord).map((candidate, index) =>
          compact({
            name: stringAt(candidate, "name"),
            confidence: valueAt(candidate, "confidence"),
            selected: selected === index,
            ambiguities: candidate.ambiguities,
            slots: summarizeInterpreterSlots(candidate.slots),
          }),
        )
      : undefined,
    rejectedProposal: rejected
      ? compact({
          disposition: stringAt(rejectedContent, "disposition"),
          selected: valueAt(rejectedContent, "selected"),
          modelOutput: stringAt(rawContent, "modelOutput", "model_output"),
        })
      : undefined,
  });
}

function summarizeInterpreterSlots(slots: unknown): unknown {
  if (!Array.isArray(slots)) return undefined;
  return slots.filter(isRecord).map((slot) =>
    compact({
      name: stringAt(slot, "name"),
      value: valueAt(slot, "value", "inferredValue", "inferred_value"),
      confidence: valueAt(slot, "confidence"),
    }),
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
    if(tele){tele.querySelectorAll('.section38-telemetry').forEach(node=>node.remove());tele.insertAdjacentHTML('afterbegin',[['Probe measurements',telemetry.measurements],['Probe failures',telemetry.failures],['Abstentions',telemetry.abstentions],['Clarifications',telemetry.clarifications],['Rejected teacher-off claims',telemetry.teacherOffViolationsRejected],['Rejected undeclared repeats',telemetry.duplicateMeasurementsRejected],['Rejected cohort leakage',telemetry.cohortLeakageRejected]].map(pair=>'<div class="card section38-telemetry"><span class="muted">'+esc(pair[0])+'</span><span class="value">'+esc(pair[1])+'</span></div>').join(''))}
  }catch(error){/* The normal inspector refresh reports transport failures. */}
}
setTimeout(renderSection38Telemetry,50);
document.getElementById('refresh')?.addEventListener('click',()=>setTimeout(renderSection38Telemetry,100));
</script>`;

const LEGACY_INDEX_HTML = `<!doctype html>
<html lang="en"><head><meta charset="utf-8" /><meta name="viewport" content="width=device-width, initial-scale=1" /><title>Spoon Inspector</title><style>
:root{color-scheme:dark;font-family:ui-sans-serif,system-ui,sans-serif;background:#101114;color:#eceef2}body{max-width:1180px;margin:0 auto;padding:32px 20px 60px}h1{margin:0 0 8px;letter-spacing:-.03em}h2{margin-top:32px}.muted{color:#9aa0ad}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:12px}.card,table,details{background:#191b21;border:1px solid #2b2e38;border-radius:12px}.card{padding:16px}.value{display:block;font-size:28px;font-weight:700;margin-top:6px}table{width:100%;border-collapse:separate;border-spacing:0;overflow:hidden}th,td{text-align:left;padding:10px 12px;border-bottom:1px solid #2b2e38;vertical-align:top}tr:last-child td{border-bottom:0}th{color:#9aa0ad;font-size:12px;text-transform:uppercase;letter-spacing:.08em}code,pre{color:#b9c6ff;white-space:pre-wrap;overflow-wrap:anywhere}.error{color:#ff9b9b}button{background:#b9c6ff;color:#101114;border:0;border-radius:8px;padding:8px 12px;cursor:pointer}.metric-card{min-height:122px}.metric-head{display:flex;justify-content:space-between;gap:8px;align-items:start}.metric-status{border:1px solid #4b5060;border-radius:999px;color:#c5cad7;font-size:11px;padding:3px 7px;white-space:nowrap}.metric-status.measured{border-color:#6cc58b;color:#9de5b2}.metric-status.partial{border-color:#d9ad61;color:#f2c77f}.metric-detail{display:block;color:#9aa0ad;font-size:13px;line-height:1.4;margin-top:10px}#episode-detail{margin-top:14px}.narrative{padding:18px}.narrative h3{margin:22px 0 8px}.narrative h3:first-child{margin-top:0}.narrative dl{display:grid;grid-template-columns:minmax(130px,220px) 1fr;gap:8px 16px;margin:0}.narrative dt{color:#9aa0ad}.narrative dd{margin:0}details{margin-top:14px;padding:12px}summary{cursor:pointer}.episode-row{cursor:pointer}.episode-row:hover{background:#222633}
</style></head><body>
<h1>Spoon Inspector</h1><p class="muted">A local read-only view of the graph, episodes, and flywheel metrics.</p><button id="refresh">Refresh</button><p id="status" class="muted"></p>
<section><h2>Section 38 metric slots</h2><p class="muted">Statuses describe what the current server actually measures; uninstrumented slots are intentionally not scored.</p><div id="metrics" class="grid"></div></section><section><h2>Raw telemetry</h2><div id="telemetry" class="grid"></div></section><section><h2>Knowledge</h2><div id="knowledge"></div></section><section><h2>Recent episodes</h2><p class="muted">Select an episode for the redacted, read-only “What happened?” narrative.</p><div id="episodes"></div><div id="episode-detail"></div></section>
<script>
const $=id=>document.getElementById(id),esc=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])),get=async path=>{const r=await fetch(path);if(!r.ok)throw new Error(await r.text());return r.json()},pretty=value=>value===undefined?'not recorded':esc(typeof value==='string'?value:JSON.stringify(value)),cell=value=>'<code>'+pretty(value)+'</code>',fieldRows=value=>Object.entries(value||{}).filter(([,v])=>v!==undefined&&v!==null&&v!=='').map(([key,v])=>'<dt>'+esc(key.replace(/([A-Z])/g,' $1'))+'</dt><dd>'+pretty(v)+'</dd>').join('');
async function showEpisode(id){$('episode-detail').innerHTML='<p class="muted">Loading episode…</p>';try{const detail=await get('/api/episodes/'+encodeURIComponent(id)),n=detail.narrative,teacher=n.teacher||{},validation=teacher.validation||{};$('episode-detail').innerHTML='<article class="card narrative"><h3>What happened?</h3><dl>'+fieldRows({request:n.request,escalation:n.escalation,learnedOrReused:n.learning&&n.learning.summary,prediction:n.prediction,observation:n.observation,evaluation:n.evaluation,cost:n.cost,abstentionReason:n.abstentionReason})+'</dl><h3>Teacher</h3><dl>'+fieldRows({used:teacher.used?'yes':'no',provider:teacher.provider,model:teacher.model,source:teacher.source,proposalSummary:teacher.proposalSummary,proposal:teacher.proposal,providerError:teacher.providerError,validationStatus:validation.status,validationChecks:validation.checks})+'</dl><h3>Learning</h3><dl>'+fieldRows(n.learning)+'</dl><details><summary>Redacted raw JSON</summary><pre>'+pretty(detail.raw)+'</pre></details></article>'}catch(error){$('episode-detail').innerHTML='<p class="error">'+esc(error.message||error)+'</p>'}}
async function refresh(){$('status').textContent='Refreshing…';try{const[metrics,concepts,procedures,episodes]=await Promise.all([get('/api/metrics'),get('/api/concepts'),get('/api/procedures'),get('/api/episodes')]),i=metrics.intuition,p=metrics.phase6||{},groundedRatio=i.supervisionTasks>0?(i.groundedTasks/i.supervisionTasks*100).toFixed(1)+'%':'No tasks yet',skills=p.managedSkillRecordsExamined??0,slots=[['1. Compounding','not-instrumented','Not instrumented','Needs cost of the Nth skill over a comparable task sequence.'],['2. Transfer','partial','Partial evidence','Persisted transfer wins: '+(p.transferEligibleSkillVerdicts??0)+' among '+skills+' examined skill records. This is promotion-gate evidence, not held-out task-family coverage.'],['3. Per-domain weaning','partial','Partial evidence','Teacher-request episodes: '+(p.teacherInteractionEpisodes??0)+'; successful teacher-free episodes: '+(p.teacherFreeSuccesses??0)+'; teacher-assisted successes: '+(p.teacherAssistedSuccesses??0)+'. No domains or comparable time cohorts are recorded.'],['4. Trace compression','not-instrumented','Not instrumented','Needs repeated task-family traces over time.'],['5. Rung distribution','measured','Measured',metrics.rungDistribution.length?'Current episode distribution is available.':'No episode rung data yet.'],['6. No regression','partial','Partial evidence','Verified baselines: '+metrics.verifiedAnswerCount+'; preserved replay verdicts: '+(p.replayPreservedSkillVerdicts??0)+'; regression verdicts: '+(p.replayRegressions??0)+'. No fresh full-suite replay is implied.'],['7. Attribution accuracy','not-instrumented','Not instrumented','Needs injected-fault outcomes and credit comparisons.'],['8. Attribution cost','not-instrumented','Not instrumented','Needs attribution and total-cost traces together.'],['9. Teacher ablation','not-instrumented','Not instrumented','Needs task-history replay with the teacher disconnected.'],['10. Grounding drift','partial','Partial signal','Grounded supervision share: '+groundedRatio+' (not a belief-level measure).'],['11. Abstraction survival','partial','Partial evidence','Recorded post-promotion success: '+(p.postPromotionSkillSuccesses??0)+' of '+(p.postPromotionSkillUses??0)+' uses across '+(p.currentlyPromotedSkills??0)+' currently promoted skills. Zero is not evidence of non-survival.'],['12. Calibration','not-instrumented','Not instrumented','Needs confidence values paired with observed correctness.']];$('metrics').innerHTML=slots.map(([label,statusClass,status,detail])=>'<div class="card metric-card"><div class="metric-head"><span class="muted">'+label+'</span><span class="metric-status '+statusClass+'">'+status+'</span></div><span class="metric-detail">'+esc(detail)+'</span></div>').join('');$('telemetry').innerHTML=[['Episodes',metrics.episodeCount],['Teacher requests',p.teacherInteractionEpisodes??0],['Verified baselines',metrics.verifiedAnswerCount],['Preserved replay verdicts',p.replayPreservedSkillVerdicts??0],['Transfer wins',p.transferEligibleSkillVerdicts??0],['Post-promotion successes',p.postPromotionSkillSuccesses??0],['Indexed docs',i.indexedDocuments],['Recall queries',i.retrievalQueries],['Candidates examined',i.candidatesExamined],['Ranking examples',i.rankingExamples],['Grounded tasks',i.groundedTasks]].map(([label,value])=>'<div class="card"><span class="muted">'+label+'</span><span class="value">'+esc(value)+'</span></div>').join('');$('knowledge').innerHTML='<table><thead><tr><th>Type</th><th>Name</th><th>Id</th></tr></thead><tbody>'+concepts.map(c=>'<tr><td>Concept</td><td>'+esc(c.name)+'</td><td>'+cell(c.id)+'</td></tr>').concat(procedures.map(p=>'<tr><td>Procedure</td><td>'+esc(p.name)+'</td><td>'+cell(p.id)+'</td></tr>')).join('')+'</tbody></table>';$('episodes').innerHTML='<table><thead><tr><th>Situation</th><th>Disposition</th><th>Rung</th><th>Result</th></tr></thead><tbody>'+episodes.map(e=>'<tr class="episode-row" data-episode-id="'+esc(e.id)+'"><td>'+esc(e.situation)+'</td><td>'+cell(e.evaluation?.success===true?'success':e.evaluation?.success===false?'failure':'unverified')+'</td><td>'+cell(e.cost?.rungReached??e.cost?.rung_reached)+'</td><td>'+cell(e.observedResult??e.observed_result)+'</td></tr>').join('')+'</tbody></table>';document.querySelectorAll('[data-episode-id]').forEach(row=>row.addEventListener('click',()=>showEpisode(row.dataset.episodeId)));$('status').textContent='Updated '+new Date().toLocaleTimeString()}catch(error){$('status').innerHTML='<span class="error">'+esc(error.message||error)+'</span>'}}
$('refresh')?.addEventListener('click',refresh);refresh();
</script></body></html>`;

const INDEX_HTML = `<!doctype html>
<html lang="en"><head><meta charset="utf-8" /><meta name="viewport" content="width=device-width, initial-scale=1" /><title>Spoon Inspector</title><style>
:root{color-scheme:dark;font-family:ui-sans-serif,system-ui,sans-serif;background:#101114;color:#eceef2}body{max-width:1240px;margin:0 auto;padding:32px 20px 60px}h1{margin:0 0 8px;letter-spacing:-.03em}h2{margin-top:32px}.muted{color:#9aa0ad}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:12px}.card,table,details{background:#191b21;border:1px solid #2b2e38;border-radius:12px}.card{padding:16px}.value{display:block;font-size:28px;font-weight:700;margin-top:6px}table{width:100%;border-collapse:separate;border-spacing:0;overflow:hidden}th,td{text-align:left;padding:10px 12px;border-bottom:1px solid #2b2e38;vertical-align:top}tr:last-child td{border-bottom:0}th{color:#9aa0ad;font-size:12px;text-transform:uppercase;letter-spacing:.08em}code,pre{color:#b9c6ff;white-space:pre-wrap;overflow-wrap:anywhere}.error{color:#ff9b9b}button{background:#b9c6ff;color:#101114;border:0;border-radius:8px;padding:8px 12px;cursor:pointer}button.secondary{background:#2b2e38;color:#eceef2}.controls{display:flex;flex-wrap:wrap;gap:8px;align-items:center}.controls input,.controls select{background:#101114;color:#eceef2;border:1px solid #4b5060;border-radius:8px;padding:8px}.metric-card{min-height:122px}.metric-head{display:flex;justify-content:space-between;gap:8px;align-items:start}.metric-status{border:1px solid #4b5060;border-radius:999px;color:#c5cad7;font-size:11px;padding:3px 7px;white-space:nowrap}.metric-status.measured{border-color:#6cc58b;color:#9de5b2}.metric-status.partial{border-color:#d9ad61;color:#f2c77f}.metric-detail{display:block;color:#9aa0ad;font-size:13px;line-height:1.4;margin-top:10px}.panel{margin-top:14px}.narrative{padding:18px}.narrative h3{margin:22px 0 8px}.narrative h3:first-child{margin-top:0}.narrative dl{display:grid;grid-template-columns:minmax(130px,220px) 1fr;gap:8px 16px;margin:0}.narrative dt{color:#9aa0ad}.narrative dd{margin:0}details{margin-top:14px;padding:12px}summary{cursor:pointer}.selectable{cursor:pointer}.selectable:hover{background:#222633}.graph{display:grid;grid-template-columns:minmax(240px,1fr) minmax(320px,1.5fr);gap:14px}.scroll{overflow:auto;max-height:420px}.episode-split{display:grid;grid-template-columns:minmax(280px,1fr) minmax(380px,2fr);gap:14px;margin-top:14px}.episode-list{overflow:auto;max-height:600px;min-width:0}.episode-list table{table-layout:fixed}.episode-detail-pane{overflow:auto;max-height:600px;min-width:0}.episode-list .selectable.active{background:#222633;border-left:3px solid #b9c6ff}.decision-flow{margin:8px 0;padding:0;list-style:none}.decision-step{position:relative;padding:8px 12px 8px 28px;margin:0;font-size:13px;line-height:1.5}.decision-step:before{content:'';position:absolute;left:8px;top:0;bottom:0;width:2px;background:#2b2e38}.decision-step:first-child:before{top:50%}.decision-step:last-child:before{bottom:50%}.decision-step:after{content:'';position:absolute;left:4px;top:50%;transform:translateY(-50%);width:10px;height:10px;border-radius:50%;border:2px solid #4b5060;background:#191b21;z-index:1}.decision-step.step-succeeded:after{border-color:#6cc58b;background:#6cc58b}.decision-step.step-failed:after{border-color:#ff9b9b;background:#ff9b9b}.decision-step .step-rung{display:inline-block;font-size:11px;padding:1px 6px;border-radius:4px;background:#2b2e38;color:#9aa0ad;margin-right:6px}.decision-step.step-succeeded .step-rung{background:#1a3a2a;color:#6cc58b}.decision-step.step-failed .step-rung{background:#3a1a1a;color:#ff9b9b}.selection-card{margin:8px 0;padding:12px 16px;background:#101114;border-radius:8px;border:1px solid #2b2e38;border-left:3px solid #b9c6ff}@media(max-width:720px){.graph{grid-template-columns:1fr}.episode-split{grid-template-columns:1fr}.narrative dl{grid-template-columns:1fr;gap:3px 0}}
</style></head><body>
<h1>Spoon Inspector</h1><p class="muted">A local, read-only view of knowledge, evidence, and the learning flywheel. Nothing here mutates the graph.</p><div class="controls"><button id="refresh">Refresh</button><span id="status" class="muted"></span></div>
<section><h2>Section 38 metric slots</h2><p class="muted">Statuses describe what the current server actually measures; uninstrumented slots are intentionally not scored.</p><div id="metrics" class="grid"></div></section>
<section><h2>Raw telemetry</h2><div id="telemetry" class="grid"></div></section>
<section><h2>Knowledge graph</h2><p class="muted">The graph is bounded for safety and legibility. Relationships are shown when the server exposes the read-only relationship collection.</p><div class="graph"><div id="knowledge"></div><div id="graph-edges" class="card scroll"></div></div><div id="procedure-detail" class="panel"></div></section>
<section><h2>Contradictions</h2><p class="muted">Held disagreements stay visible with their claims, supporting episodes, scopes, and any later refinement.</p><div id="contradictions"></div><div id="contradiction-detail" class="panel"></div></section>
<section><h2>Intrinsics reference</h2><p class="muted">All built-in operations available in procedure bodies via <code>{kind:'intrinsic',version:1,op:'&lt;name&gt;',args:[...]}</code>.</p><div class="controls"><input id="intrinsic-search" type="search" placeholder="Filter intrinsics..." /><select id="intrinsic-category"><option value="">All categories</option></select></div><div class="episode-split"><div id="intrinsic-list" class="episode-list"></div><div id="intrinsic-detail" class="episode-detail-pane"></div></div></section>
<section><h2>Episodes</h2><p class="muted">Search or filter episodes, then open one for provenance. Replay is explicitly read-only and remains redacted.</p><div class="controls"><input id="episode-search" type="search" placeholder="Search request or action" /><select id="episode-outcome"><option value="">Any outcome</option><option value="success">Success</option><option value="failure">Failure</option></select><button id="episode-filter" class="secondary">Apply</button></div><div class="episode-split"><div id="episodes" class="episode-list"></div><div id="episode-detail" class="episode-detail-pane"></div></div></section>
<script>
const $=sel=>document.getElementById(sel)||document.querySelector(sel),esc=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c])),get=async path=>{const r=await fetch(path);if(!r.ok)throw new Error(await r.text());return r.json()},pretty=value=>value===undefined?'not recorded':esc(typeof value==='string'?value:JSON.stringify(value)),cell=value=>'<code>'+pretty(value)+'</code>';
function renderValue(v){if(v===undefined)return'<span class="muted">not recorded</span>';if(v===null)return'<span class="muted">null</span>';if(typeof v!=='object')return'<code>'+esc(String(v))+'</code>';if(Array.isArray(v)){if(!v.length)return'<span class="muted">[]</span>';if(v.every(function(x){return typeof x!=='object'||x===null}))return'<code>'+esc(JSON.stringify(v))+'</code>';return'<div style="margin:4px 0">'+v.map(function(item,i){return'<div style="margin:4px 0;padding:8px 12px;background:#101114;border-radius:8px;border:1px solid #2b2e38">'+renderValue(item)+'</div>'}).join('')+'</div>';}if(v.requires||v.promises||v.fails_when||v.failsWhen)return renderContract(v);if(v.Var||v.Literal||v.BinOp||v.Call||v.Let||v.If||v.Intrinsic||v.ListExpr||v.Index||v.Field||v.UnaryOp||v.CallExact)return'<div style="padding:6px 10px;background:#101114;border-radius:6px;border:1px solid #2b2e38;font-family:ui-monospace,monospace;font-size:13px;line-height:1.6;display:inline-block">'+renderExpr(v)+'</div>';if(v.check&&v.description)return'<div style="color:#c5cad7;font-size:13px">'+esc(v.description)+'</div><div style="font-family:ui-monospace,monospace;font-size:13px">'+renderExpr(v.check)+'</div>';var keys=Object.keys(v);if(!keys.length)return'<span class="muted">{}</span>';return'<dl style="margin:0;font-size:13px">'+keys.filter(function(k){return v[k]!==undefined&&v[k]!==null&&v[k]!==''}).map(function(k){return'<dt style="color:#9aa0ad">'+esc(k.replace(/([A-Z])/g,' $1'))+'</dt><dd style="margin:0 0 6px">'+renderValue(v[k])+'</dd>'}).join('')+'</dl>';}
function fieldRows(value){return Object.entries(value||{}).filter(function(e){return e[1]!==undefined&&e[1]!==null&&e[1]!==''}).map(function(e){return'<dt>'+esc(e[0].replace(/([A-Z])/g,' $1'))+'</dt><dd>'+renderValue(e[1])+'</dd>'}).join('');}
function renderProposal(proposal){if(!proposal||typeof proposal!=='object')return pretty(proposal);const p=proposal;let html='';if(p.params||p.parameters){html+='<h4 style="margin-top:12px">Parameters</h4>'+renderParams(p.params||p.parameters);}if(p.body||p.expression){html+='<h4 style="margin-top:12px">Body</h4><div style="padding:10px 14px;background:#101114;border-radius:8px;border:1px solid #2b2e38;font-family:ui-monospace,monospace;font-size:13px;line-height:1.6">'+renderExpr(p.body||p.expression)+'</div>';}if(p.contract){html+='<h4 style="margin-top:12px">Contract</h4>'+renderContract(p.contract);}if(!html)return'<pre>'+esc(JSON.stringify(proposal,null,2))+'</pre>';return html;}
function renderDecisionPath(n){var steps=n.decisionPath,sel=n.selectionReason;if(!steps&&!sel)return'';var html='<h3>Decision path</h3>';if(steps&&steps.length){html+='<ul class="decision-flow">';for(var i=0;i<steps.length;i++){var s=steps[i],cls=s.status==='Failed'?'step-failed':'step-succeeded';html+='<li class="decision-step '+cls+'"><span class="step-rung">'+esc(s.rung||'?')+'</span>'+esc(s.description||'')+(s.error?' <span class="error">('+esc(s.error)+')</span>':'')+(s.procedure?' <code>'+esc(s.procedure)+'</code>':'')+'</li>';}html+='</ul>';}if(sel){html+='<div class="selection-card"><strong>Why this procedure?</strong><div style="margin-top:6px;color:#c5cad7">'+esc(sel.summary||'')+'</div><dl style="margin:8px 0 0;font-size:13px">';if(sel.path)html+='<dt style="color:#9aa0ad">Path</dt><dd><code>'+esc(sel.path)+'</code></dd>';if(sel.procedure)html+='<dt style="color:#9aa0ad">Procedure</dt><dd><code>'+esc(sel.procedure)+(sel.version!=null?'@'+esc(sel.version):'')+'</code></dd>';if(sel.concept)html+='<dt style="color:#9aa0ad">Concept</dt><dd><code>'+esc(sel.concept)+'</code></dd>';if(sel.source)html+='<dt style="color:#9aa0ad">Source</dt><dd>'+esc(sel.source)+'</dd>';if(sel.disposition)html+='<dt style="color:#9aa0ad">Disposition</dt><dd>'+esc(sel.disposition)+'</dd>';if(sel.chain)html+='<dt style="color:#9aa0ad">Chain</dt><dd>'+esc(Array.isArray(sel.chain)?sel.chain.join(' -> '):sel.chain)+'</dd>';if(sel.slotBindings)html+='<dt style="color:#9aa0ad">Slot bindings</dt><dd>'+renderValue(sel.slotBindings)+'</dd>';html+='</dl></div>';}return html;}
function renderInterpreter(interp){if(!interp||!interp.used)return'';var html='<h3>Interpreter</h3><dl>'+fieldRows({source:interp.source,status:interp.status,provider:interp.provider,model:interp.model,disposition:interp.disposition,selected:interp.selected,candidateCount:interp.candidateCount,priorTurnCount:interp.priorTurnCount,providerError:interp.providerError,rejection:interp.rejection})+'</dl>';if(interp.candidates&&interp.candidates.length){html+='<h4 style="margin-top:12px">Candidates</h4>';for(var i=0;i<interp.candidates.length;i++){var c=interp.candidates[i];html+='<div class="selection-card" style="border-left-color:'+(c.selected?'#d9ad61':'#2b2e38')+'"><code style="color:#d9ad61">'+esc(c.name||'?')+'</code>'+(c.selected?' <span class="muted">selected</span>':'')+(c.confidence!=null?' <span class="muted">'+esc(c.confidence)+'</span>':'');if(c.slots&&c.slots.length)html+='<div class="muted" style="margin-top:4px">'+c.slots.map(function(s){return esc(s.name)+'='+esc(JSON.stringify(s.value))}).join(', ')+'</div>';if(c.ambiguities&&c.ambiguities.length)html+='<div class="error" style="margin-top:4px">ambiguous: '+esc(c.ambiguities.join(', '))+'</div>';html+='</div>';}}if(interp.rejectedProposal)html+='<h4 style="margin-top:12px">Rejected proposal</h4><dl>'+fieldRows(interp.rejectedProposal)+'</dl>';return html;}
async function showEpisode(id){document.querySelectorAll('[data-episode-id]').forEach(function(r){r.classList.toggle('active',r.dataset.episodeId===id)});$('episode-detail').innerHTML='<p class="muted">Loading episode...</p>';try{const detail=await get('/api/episodes/'+encodeURIComponent(id)),n=detail.narrative||{},teacher=n.teacher||{},validation=teacher.validation||{},proposalObj=teacher.proposal;$('episode-detail').innerHTML='<article class="card narrative"><h3>What happened?</h3><dl>'+fieldRows({request:n.request,prediction:n.prediction,observation:n.observation,evaluation:n.evaluation,cost:n.cost,abstentionReason:n.abstentionReason})+'</dl>'+renderDecisionPath(n)+renderInterpreter(n.interpreter)+(teacher.used?'<h3>Teacher and provenance</h3><dl>'+fieldRows({provider:teacher.provider,model:teacher.model,source:teacher.source,proposalKind:teacher.proposalKind,proposalSummary:teacher.proposalSummary,providerError:teacher.providerError,validationStatus:validation.status,validationChecks:validation.checks})+'</dl>':'')+(proposalObj?'<h3>Proposal</h3>'+renderProposal(proposalObj):'')+'<h3>Learning</h3><dl>'+fieldRows(n.learning)+'</dl><button class="secondary" id="replay-button">Replay read-only</button><div id="replay-result"></div><details><summary>Redacted raw JSON</summary><pre>'+esc(JSON.stringify(detail.raw,null,2))+'</pre></details></article>';$('replay-button')?.addEventListener('click',async()=>{const output=$('replay-result');if(!output)return;output.innerHTML='<p class="muted">Replaying...</p>';try{const replay=await get('/api/episodes/'+encodeURIComponent(id)+'/replay');output.innerHTML='<p>Replay result (read-only):</p><pre>'+pretty(replay)+'</pre>'}catch(error){output.innerHTML='<p class="error">'+esc(error.message||error)+'</p>'}})}catch(error){$('episode-detail').innerHTML='<p class="error">'+esc(error.message||error)+'</p>'}}
function renderExpr(e){if(e==null)return'<span class="muted">null</span>';if(typeof e!=='object')return'<span style="color:#f2c77f">'+esc(JSON.stringify(e))+'</span>';if('Var'in e)return'<span style="color:#9de5b2;font-weight:600">'+esc(e.Var)+'</span>';if('Literal'in e)return'<span style="color:#f2c77f">'+esc(JSON.stringify(e.Literal))+'</span>';if('BinOp'in e){const b=e.BinOp;return'<span class="expr-group">('+renderExpr(b.left)+' <span style="color:#b9c6ff;font-weight:600">'+esc(b.op)+'</span> '+renderExpr(b.right)+')</span>';}if('UnaryOp'in e){const u=e.UnaryOp;return'<span style="color:#b9c6ff">'+esc(u.op)+'</span>('+renderExpr(u.operand)+')';}if('Intrinsic'in e){const i=e.Intrinsic;return'<span style="color:#b9c6ff;font-weight:600">'+esc(i.op)+'</span>('+(i.args||[]).map(renderExpr).join(', ')+')';}if('ListExpr'in e)return'['+e.ListExpr.map(renderExpr).join(', ')+']';if('Index'in e){const x=e.Index;return renderExpr(x.collection)+'['+renderExpr(x.index)+']';}if('Field'in e){const f=e.Field;return renderExpr(f.object)+'.<span style="color:#c5cad7">'+esc(f.field)+'</span>';}if('If'in e){const f=e.If;return'<span style="color:#b9c6ff">if</span> '+renderExpr(f.condition)+' <span style="color:#b9c6ff">then</span> '+renderExpr(f.then||f.consequent)+' <span style="color:#b9c6ff">else</span> '+renderExpr(f.else_||f.alternate||f.otherwise);}if('Let'in e){const l=e.Let;return'<span style="color:#b9c6ff">let</span> <span style="color:#9de5b2">'+esc(l.name)+'</span> = '+renderExpr(l.value)+' <span style="color:#b9c6ff">in</span> '+renderExpr(l.body);}if('Call'in e){const c=e.Call;return'<span style="color:#d9ad61">'+esc(c.procedure||c.alias)+'</span>('+(c.args||c.inputs||[]).map(renderExpr).join(', ')+')';}return'<code>'+esc(JSON.stringify(e,null,1))+'</code>';}
function renderConditions(label,conditions){if(!conditions||!conditions.length)return'<div style="margin:4px 0"><span class="muted">'+esc(label)+':</span> <span class="muted">none</span></div>';return'<div style="margin:8px 0"><strong style="color:#9aa0ad;font-size:12px;text-transform:uppercase;letter-spacing:.06em">'+esc(label)+'</strong>'+conditions.map(function(c){return'<div style="margin:6px 0;padding:10px 14px;background:#101114;border-radius:8px;border:1px solid #2b2e38"><div style="color:#c5cad7;font-size:13px;margin-bottom:6px">'+esc(c.description)+'</div><div style="font-family:ui-monospace,monospace;font-size:13px;line-height:1.6">'+renderExpr(c.check)+'</div></div>';}).join('')+'</div>';}
function renderParams(params){if(!Array.isArray(params)||!params.length)return'<span class="muted">none</span>';return'<table style="margin:4px 0"><thead><tr><th>Name</th><th>Type</th><th>Description</th></tr></thead><tbody>'+params.map(function(p){return'<tr><td><code style="color:#9de5b2">'+esc(p.name)+'</code></td><td><code>'+esc(p.valueType||p.value_type||'any')+'</code></td><td>'+esc(p.description||'')+'</td></tr>';}).join('')+'</tbody></table>';}
function renderContract(contract){if(!contract)return'<p class="muted">No contract.</p>';const req=contract.requires||[];const prom=contract.promises||[];const fails=contract.fails_when||contract.failsWhen||[];return renderConditions('Requires',req)+renderConditions('Promises',prom)+renderConditions('Fails when',fails);}
async function showProcedure(id){$('procedure-detail').innerHTML='<p class="muted">Loading procedure…</p>';try{const detail=await get('/api/procedures/'+encodeURIComponent(id)),p=detail.procedure||{};const body=p.body||p.expression;$('procedure-detail').innerHTML='<article class="card narrative"><h3>Procedure: '+esc(p.name||p.id)+'</h3><dl>'+fieldRows({id:p.id,version:p.version,lifecycle:p.lifecycle,concept:p.concept})+'</dl><h4 style="margin-top:16px">Parameters</h4>'+renderParams(p.params||p.parameters)+(body?'<h4 style="margin-top:16px">Body</h4><div style="padding:10px 14px;background:#101114;border-radius:8px;border:1px solid #2b2e38;font-family:ui-monospace,monospace;font-size:13px;line-height:1.6">'+renderExpr(body)+'</div>':'')+'<h4 style="margin-top:16px">Contract</h4>'+renderContract(p.contract)+'<h3>Version history</h3><p class="muted">'+(detail.historyAvailable?(detail.versions.length+' recorded version(s).'): 'Version history is not exposed by this server.')+'</p>'+renderValue(detail.versions)+'</article>'}catch(error){$('procedure-detail').innerHTML='<p class="error">'+esc(error.message||error)+'</p>';}}
async function showContradiction(id){$('contradiction-detail').innerHTML='<p class="muted">Loading contradiction…</p>';try{const value=await get('/api/contradictions/'+encodeURIComponent(id));$('contradiction-detail').innerHTML='<article class="card narrative"><h3>Contradiction '+esc(id)+'</h3>'+renderValue(value)+'</article>'}catch(error){$('contradiction-detail').innerHTML='<p class="error">'+esc(error.message||error)+'</p>'}}
function renderKnowledge(graph,procedures){const names=new Map((graph.nodes||[]).map(n=>[n.id,n.name]));$('knowledge').innerHTML='<div class="card scroll"><strong>'+esc((graph.nodes||[]).length)+' nodes</strong><table><thead><tr><th>Type</th><th>Name</th><th>Version</th></tr></thead><tbody>'+((graph.nodes||[]).map(n=>'<tr><td>'+esc(n.kind)+'</td><td>'+esc(n.name)+'</td><td>'+cell(n.version)+'</td></tr>').join('')||'<tr><td colspan="3" class="muted">No knowledge yet.</td></tr>')+'</tbody></table></div>';$("graph-edges").innerHTML='<strong>Bounded edges</strong><p class="muted">'+esc((graph.edges||[]).length)+' edge(s); max '+esc(graph.maxEdges)+'</p>'+((graph.edges||[]).map(e=>'<p><code>'+esc(names.get(e.source)||e.source)+'</code> → <code>'+esc(names.get(e.target)||e.target)+'</code> <span class="muted">('+esc(e.kind)+')</span></p>').join('')||'<p class="muted">No relationships available.</p>');const procedureRows=(procedures||[]).map(p=>'<tr class="selectable" data-procedure-id="'+esc(p.id)+'"><td>Procedure</td><td>'+esc(p.name)+'</td><td>'+cell(p.version)+'</td></tr>').join('');$('knowledge').innerHTML+='<div class="card"><h3>Procedures</h3><table><thead><tr><th>Type</th><th>Name</th><th>Version</th></tr></thead><tbody>'+procedureRows+'</tbody></table></div>';document.querySelectorAll('[data-procedure-id]').forEach(row=>row.addEventListener('click',()=>showProcedure(row.dataset.procedureId)))}
async function refreshEpisodes(){const q=$('episode-search').value.trim(),outcome=$('episode-outcome').value,path='/api/episodes'+(q?'?q='+encodeURIComponent(q):'');const episodes=await get(path);const rows=(episodes||[]).map(e=>{const status=e.evaluation?.success===true?'success':e.evaluation?.success===false?'failure':'unverified';const color=status==='success'?'#6cc58b':status==='failure'?'#ff9b9b':'#9aa0ad';return !outcome||status===outcome?'<tr class="selectable" data-episode-id="'+esc(e.id)+'"><td style="max-width:220px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="'+esc(e.situation)+'">'+esc(e.situation)+'</td><td><span style="color:'+color+'">'+esc(status)+'</span></td><td>'+cell(e.cost?.rungReached??e.cost?.rung_reached)+'</td></tr>':''}).join('');$('episodes').innerHTML='<table><thead><tr><th>Request</th><th>Status</th><th>Rung</th></tr></thead><tbody>'+(rows||'<tr><td colspan="3" class="muted">No matching episodes.</td></tr>')+'</tbody></table>';if(!$('episode-detail').innerHTML)$('episode-detail').innerHTML='<div class="card" style="display:flex;align-items:center;justify-content:center;min-height:200px"><p class="muted">Select an episode to view details</p></div>';document.querySelectorAll('[data-episode-id]').forEach(row=>row.addEventListener('click',()=>showEpisode(row.dataset.episodeId)))}
async function refresh(){$('status').textContent='Refreshing…';try{const[metrics,graph,procedures,contradictions]=await Promise.all([get('/api/metrics'),get('/api/knowledge'),get('/api/procedures'),get('/api/contradictions')]),i=metrics.intuition,p=metrics.phase6||{},groundedRatio=i.supervisionTasks>0?(i.groundedTasks/i.supervisionTasks*100).toFixed(1)+'%':'No tasks yet',skills=p.managedSkillRecordsExamined??0,slots=[['1. Compounding','not-instrumented','Not instrumented','Needs cost of the Nth skill over a comparable task sequence.'],['2. Transfer','partial','Partial evidence','Persisted transfer wins: '+(p.transferEligibleSkillVerdicts??0)+' among '+skills+' examined skill records. This is promotion-gate evidence, not held-out task-family coverage.'],['3. Per-domain weaning','partial','Partial evidence','Teacher-request episodes: '+(p.teacherInteractionEpisodes??0)+'; successful teacher-free episodes: '+(p.teacherFreeSuccesses??0)+'; teacher-assisted successes: '+(p.teacherAssistedSuccesses??0)+'. No domains or comparable time cohorts are recorded.'],['4. Trace compression','not-instrumented','Not instrumented','Needs repeated task-family traces over time.'],['5. Rung distribution','measured','Measured',metrics.rungDistribution.length?'Current episode distribution is available.':'No episode rung data yet.'],['6. No regression','partial','Partial evidence','Verified baselines: '+metrics.verifiedAnswerCount+'; preserved replay verdicts: '+(p.replayPreservedSkillVerdicts??0)+'; regression verdicts: '+(p.replayRegressions??0)+'. No fresh full-suite replay is implied.'],['7. Attribution accuracy','not-instrumented','Not instrumented','Needs injected-fault outcomes and credit comparisons.'],['8. Attribution cost','not-instrumented','Not instrumented','Needs attribution and total-cost traces together.'],['9. Teacher ablation','not-instrumented','Not instrumented','Needs task-history replay with the teacher disconnected.'],['10. Grounding drift','partial','Partial signal','Grounded supervision share: '+groundedRatio+' (not a belief-level measure).'],['11. Abstraction survival','partial','Partial evidence','Recorded post-promotion success: '+(p.postPromotionSkillSuccesses??0)+' of '+(p.postPromotionSkillUses??0)+' uses across '+(p.currentlyPromotedSkills??0)+' currently promoted skills. Zero is not evidence of non-survival.'],['12. Calibration','not-instrumented','Not instrumented','Needs confidence values paired with observed correctness.']];$('metrics').innerHTML=slots.map(([label,statusClass,status,detail])=>'<div class="card metric-card"><div class="metric-head"><span class="muted">'+label+'</span><span class="metric-status '+statusClass+'">'+status+'</span></div><span class="metric-detail">'+esc(detail)+'</span></div>').join('');$('telemetry').innerHTML=[['Episodes',metrics.episodeCount],['Teacher requests',p.teacherInteractionEpisodes??0],['Verified baselines',metrics.verifiedAnswerCount],['Preserved replay verdicts',p.replayPreservedSkillVerdicts??0],['Transfer wins',p.transferEligibleSkillVerdicts??0],['Post-promotion successes',p.postPromotionSkillSuccesses??0],['Indexed docs',i.indexedDocuments],['Recall queries',i.retrievalQueries],['Candidates examined',i.candidatesExamined],['Ranking examples',i.rankingExamples],['Grounded tasks',i.groundedTasks]].map(([label,value])=>'<div class="card"><span class="muted">'+label+'</span><span class="value">'+esc(value)+'</span></div>').join('');renderKnowledge(graph,procedures);$('contradictions').innerHTML='<table><thead><tr><th>Id</th><th>Status</th><th>Left claim</th><th>Right claim</th></tr></thead><tbody>'+((contradictions||[]).map(c=>'<tr class="selectable" data-contradiction-id="'+esc(c.id)+'"><td>'+cell(c.id)+'</td><td>'+esc(c.status)+'</td><td>'+esc(c.left?.statement)+'</td><td>'+esc(c.right?.statement)+'</td></tr>').join('')||'<tr><td colspan="4" class="muted">No held contradictions.</td></tr>')+'</tbody></table>';document.querySelectorAll('[data-contradiction-id]').forEach(row=>row.addEventListener('click',()=>showContradiction(row.dataset.contradictionId)));await refreshEpisodes();$('status').textContent='Updated '+new Date().toLocaleTimeString()}catch(error){$('status').innerHTML='<span class="error">'+esc(error.message||error)+'</span>'}}
const INTRINSICS_CATALOG=[
{cat:'Text',ops:[
{name:'length',arity:1,desc:'Length of text, list, or map'},
{name:'text_byte_length',arity:1,desc:'Byte length of text'},
{name:'text_scalar_length',arity:1,desc:'Unicode scalar count'},
{name:'text_grapheme_length',arity:1,desc:'Grapheme cluster count'},
{name:'text_tokenize',arity:1,desc:'Split text into word tokens'},
{name:'text_split',arity:2,desc:'Split text by delimiter'},
{name:'text_join',arity:2,desc:'Join list with separator'},
{name:'text_trim',arity:1,desc:'Trim whitespace from both ends'},
{name:'text_trim_start',arity:1,desc:'Trim leading whitespace'},
{name:'text_trim_end',arity:1,desc:'Trim trailing whitespace'},
{name:'text_lowercase',arity:1,desc:'Convert to lowercase'},
{name:'text_uppercase',arity:1,desc:'Convert to uppercase'},
{name:'text_contains',arity:2,desc:'Check if text contains substring'},
{name:'text_starts_with',arity:2,desc:'Check if text starts with prefix'},
{name:'text_ends_with',arity:2,desc:'Check if text ends with suffix'},
{name:'text_replace',arity:3,desc:'Replace first occurrence of pattern'},
{name:'text_index_of',arity:2,desc:'Index of first occurrence (-1 if absent)'},
{name:'text_count',arity:2,desc:'Count non-overlapping occurrences'},
{name:'text_repeat',arity:2,desc:'Repeat text N times'},
{name:'text_concat_many',arity:1,desc:'Concatenate list of texts'},
{name:'text_url_encode',arity:1,desc:'Percent-encode for URL query'},
{name:'text_url_decode',arity:1,desc:'Percent-decode URL component'},
{name:'text_regex_capture',arity:2,desc:'Capture groups from regex match'},
{name:'text_normalize_nfc',arity:1,desc:'Unicode NFC normalization'},
{name:'text_normalize_nfd',arity:1,desc:'Unicode NFD normalization'},
{name:'text_normalize_nfkc',arity:1,desc:'Unicode NFKC normalization'},
{name:'text_normalize_nfkd',arity:1,desc:'Unicode NFKD normalization'},
{name:'text_grapheme_substring',arity:3,desc:'Substring by grapheme indices'},
{name:'text_pad_start',arity:3,desc:'Pad text on the left to target length'},
{name:'text_pad_end',arity:3,desc:'Pad text on the right to target length'},
{name:'text_substring',arity:3,desc:'Substring by byte indices'},
{name:'text_char_at',arity:2,desc:'Character at given index'},
{name:'text_format',arity:2,desc:'Template formatting: replace {key} from map'},
{name:'text_matches_regex',arity:2,desc:'Test if text matches regex pattern'},
{name:'text_regex_replace_all',arity:3,desc:'Replace all regex matches'},
{name:'text_base64_encode',arity:1,desc:'Encode text as base64'},
{name:'text_base64_decode',arity:1,desc:'Decode base64 to text'},
{name:'text_hex_encode',arity:1,desc:'Encode text bytes as hex'},
{name:'text_hex_decode',arity:1,desc:'Decode hex string to text'},
{name:'text_reverse',arity:1,desc:'Reverse text by characters'},
{name:'text_char_code',arity:1,desc:'Unicode code point of first character'},
{name:'text_from_char_code',arity:1,desc:'Character from Unicode code point'},
{name:'text_levenshtein',arity:2,desc:'Levenshtein edit distance between two texts'},
]},
{cat:'Collection',ops:[
{name:'collection_contains',arity:2,desc:'Check if list contains value'},
{name:'collection_find_index',arity:2,desc:'Index of value in list (-1 if absent)'},
{name:'collection_slice',arity:3,desc:'Sublist from start to end index'},
{name:'collection_reverse',arity:1,desc:'Reverse list order'},
{name:'collection_sort',arity:1,desc:'Sort list (natural order)'},
{name:'collection_unique',arity:1,desc:'Remove duplicate values'},
{name:'collection_flatten',arity:1,desc:'Flatten nested lists one level'},
{name:'collection_zip',arity:2,desc:'Zip two lists into pairs'},
{name:'count_equal',arity:2,desc:'Count elements equal to value'},
{name:'range',arity:3,desc:'Generate integer range [start, end) by step'},
{name:'collection_group_by',arity:2,desc:'Group list of maps by key'},
{name:'collection_sort_by',arity:2,desc:'Sort list of maps by key'},
{name:'collection_min_by',arity:2,desc:'Element with minimum value at key'},
{name:'collection_max_by',arity:2,desc:'Element with maximum value at key'},
{name:'collection_chunk',arity:2,desc:'Split list into chunks of size N'},
{name:'collection_enumerate',arity:1,desc:'List of [index, value] pairs'},
{name:'collection_any',arity:1,desc:'True if any element is truthy'},
{name:'collection_all',arity:1,desc:'True if all elements are truthy'},
{name:'collection_take',arity:2,desc:'Take first N elements'},
{name:'collection_drop',arity:2,desc:'Drop first N elements'},
{name:'collection_first',arity:1,desc:'First element (error if empty)'},
{name:'collection_last',arity:1,desc:'Last element (error if empty)'},
{name:'collection_partition',arity:2,desc:'Split into [matches, non-matches]'},
{name:'collection_repeat_value',arity:2,desc:'Create list of N copies of value'},
{name:'collection_window',arity:2,desc:'Sliding windows of size N'},
]},
{cat:'Map',ops:[
{name:'map_keys',arity:1,desc:'List of map keys'},
{name:'map_values',arity:1,desc:'List of map values'},
{name:'map_entries',arity:1,desc:'List of [key, value] pairs'},
{name:'map_from_entries',arity:1,desc:'Build map from [key, value] pairs'},
{name:'map_set',arity:3,desc:'Set key to value in map'},
{name:'map_delete',arity:2,desc:'Remove key from map'},
{name:'map_merge',arity:2,desc:'Merge two maps (right wins)'},
{name:'map_has_key',arity:2,desc:'Check if map contains key'},
{name:'map_get_default',arity:3,desc:'Get value or default if missing'},
{name:'map_size',arity:1,desc:'Number of entries in map'},
{name:'map_filter_keys',arity:2,desc:'Keep only listed keys'},
]},
{cat:'JSON',ops:[
{name:'json_parse',arity:1,desc:'Parse JSON text into value'},
{name:'json_stringify',arity:1,desc:'Serialize value to JSON text'},
{name:'path_get',arity:2,desc:'Get value at dot-separated path'},
{name:'path_get_optional',arity:2,desc:'Get value at path or null'},
{name:'json_pointer_get',arity:2,desc:'Get value at JSON Pointer'},
{name:'json_pointer_get_optional',arity:2,desc:'Get value at pointer or null'},
{name:'json_pointer_set',arity:3,desc:'Set value at JSON Pointer'},
{name:'json_pointer_delete',arity:2,desc:'Delete value at JSON Pointer'},
]},
{cat:'Math',ops:[
{name:'numeric_abs',arity:1,desc:'Absolute value'},
{name:'numeric_sign',arity:1,desc:'Sign (-1, 0, or 1)'},
{name:'numeric_min',arity:2,desc:'Minimum of two numbers'},
{name:'numeric_max',arity:2,desc:'Maximum of two numbers'},
{name:'numeric_clamp',arity:3,desc:'Clamp value between min and max'},
{name:'numeric_floor',arity:1,desc:'Floor (round down)'},
{name:'numeric_ceil',arity:1,desc:'Ceiling (round up)'},
{name:'numeric_round',arity:1,desc:'Round to nearest integer'},
{name:'numeric_truncate',arity:1,desc:'Truncate toward zero'},
{name:'numeric_pow_int',arity:2,desc:'Integer exponentiation'},
{name:'numeric_pow_float',arity:2,desc:'Float exponentiation'},
{name:'integer_quotient',arity:2,desc:'Integer division (truncated)'},
{name:'integer_remainder',arity:2,desc:'Integer remainder'},
{name:'math_sqrt',arity:1,desc:'Square root'},
{name:'math_log',arity:1,desc:'Natural logarithm'},
{name:'math_log10',arity:1,desc:'Base-10 logarithm'},
{name:'math_log2',arity:1,desc:'Base-2 logarithm'},
{name:'math_exp',arity:1,desc:'e raised to power'},
{name:'math_sin',arity:1,desc:'Sine (radians)'},
{name:'math_cos',arity:1,desc:'Cosine (radians)'},
{name:'math_tan',arity:1,desc:'Tangent (radians)'},
{name:'math_asin',arity:1,desc:'Arcsine'},
{name:'math_acos',arity:1,desc:'Arccosine'},
{name:'math_atan',arity:1,desc:'Arctangent'},
{name:'math_atan2',arity:2,desc:'Two-argument arctangent'},
{name:'math_pi',arity:0,desc:'Pi constant (3.14159...)'},
{name:'math_e',arity:0,desc:'Euler number e (2.71828...)'},
{name:'math_is_nan',arity:1,desc:'Check if value is NaN'},
{name:'math_is_infinite',arity:1,desc:'Check if value is infinite'},
{name:'math_gcd',arity:2,desc:'Greatest common divisor'},
{name:'math_lcm',arity:2,desc:'Least common multiple'},
{name:'math_hypot',arity:2,desc:'Hypotenuse (sqrt(a^2 + b^2))'},
]},
{cat:'Random',ops:[
{name:'random_int',arity:2,desc:'Random integer in [low, high]'},
{name:'random_float',arity:0,desc:'Random float in [0.0, 1.0)'},
{name:'random_choice',arity:1,desc:'Random element from list'},
{name:'random_shuffle',arity:1,desc:'Shuffled copy of list'},
{name:'random_sample',arity:2,desc:'N unique random elements from list'},
{name:'random_uuid',arity:0,desc:'Generate UUID v4 string'},
]},
{cat:'Date/Time',ops:[
{name:'date_now',arity:0,desc:'Current UTC unix timestamp (seconds)'},
{name:'date_from_parts',arity:3,desc:'Timestamp from year, month, day'},
{name:'date_get_part',arity:2,desc:'Extract part (year/month/day/hour/minute/second/weekday)'},
{name:'date_add',arity:3,desc:'Add amount in unit to timestamp'},
{name:'date_diff',arity:3,desc:'Difference between timestamps in unit'},
{name:'date_format',arity:2,desc:'Format timestamp (%Y %m %d %H %M %S)'},
]},
{cat:'Type',ops:[
{name:'type_name',arity:1,desc:'Runtime type name of value'},
{name:'is_null',arity:1,desc:'True if value is null'},
{name:'is_bool',arity:1,desc:'True if value is boolean'},
{name:'is_int',arity:1,desc:'True if value is integer'},
{name:'is_float',arity:1,desc:'True if value is float'},
{name:'is_text',arity:1,desc:'True if value is text'},
{name:'is_list',arity:1,desc:'True if value is list'},
{name:'is_map',arity:1,desc:'True if value is map'},
{name:'is_numeric',arity:1,desc:'True if value is int or float'},
{name:'to_int',arity:1,desc:'Convert to integer'},
{name:'to_float',arity:1,desc:'Convert to float'},
{name:'to_bool',arity:1,desc:'Convert to boolean (truthiness)'},
{name:'to_text',arity:1,desc:'Convert to text'},
{name:'parse_int',arity:1,desc:'Parse text to integer'},
{name:'parse_float',arity:1,desc:'Parse text to float'},
{name:'parse_bool',arity:1,desc:'Parse text to boolean'},
]},
{cat:'Set',ops:[
{name:'set_union',arity:2,desc:'Unique elements from both lists'},
{name:'set_intersect',arity:2,desc:'Elements present in both lists'},
{name:'set_difference',arity:2,desc:'Elements in first but not second'},
{name:'set_is_subset',arity:2,desc:'True if first is subset of second'},
]},
{cat:'Bitwise',ops:[
{name:'bit_and',arity:2,desc:'Bitwise AND'},
{name:'bit_or',arity:2,desc:'Bitwise OR'},
{name:'bit_xor',arity:2,desc:'Bitwise XOR'},
{name:'bit_not',arity:1,desc:'Bitwise NOT'},
{name:'bit_shift_left',arity:2,desc:'Left shift'},
{name:'bit_shift_right',arity:2,desc:'Arithmetic right shift'},
]},
{cat:'Hash',ops:[
{name:'hash_sha256',arity:1,desc:'SHA-256 hash (hex output)'},
{name:'hash_md5',arity:1,desc:'MD5 hash (hex output)'},
]},
{cat:'Format',ops:[
{name:'numeric_to_fixed',arity:2,desc:'Format number to N decimal places'},
{name:'numeric_to_hex',arity:1,desc:'Integer to hex string'},
{name:'numeric_from_hex',arity:1,desc:'Parse hex string to integer'},
{name:'numeric_to_binary',arity:1,desc:'Integer to binary string'},
{name:'numeric_from_binary',arity:1,desc:'Parse binary string to integer'},
]},
{cat:'Control',ops:[
{name:'coalesce',arity:2,desc:'First non-null of two or more arguments'},
{name:'assert',arity:2,desc:'Return value if truthy, else error with message'},
{name:'default_if_null',arity:2,desc:'Value if not null, else default'},
]},
];
(function initIntrinsics(){
const cats=INTRINSICS_CATALOG.map(c=>c.cat);
const sel=$('intrinsic-category');
cats.forEach(c=>{const o=document.createElement('option');o.value=c;o.textContent=c+' ('+INTRINSICS_CATALOG.find(x=>x.cat===c).ops.length+')';sel.appendChild(o)});
function renderList(){
const q=($('intrinsic-search').value||'').toLowerCase();
const cat=$('intrinsic-category').value;
let ops=[];
INTRINSICS_CATALOG.forEach(c=>{if(cat&&c.cat!==cat)return;c.ops.forEach(o=>{if(q&&!o.name.includes(q)&&!o.desc.toLowerCase().includes(q))return;ops.push({...o,cat:c.cat})})});
$('intrinsic-list').innerHTML='<table><thead><tr><th>Name</th><th>Args</th><th>Category</th></tr></thead><tbody>'+(ops.map(o=>'<tr class="selectable" data-intrinsic="'+esc(o.name)+'"><td><code>'+esc(o.name)+'</code></td><td style="text-align:center">'+o.arity+'</td><td><span class="muted">'+esc(o.cat)+'</span></td></tr>').join('')||'<tr><td colspan="3" class="muted">No matching intrinsics.</td></tr>')+'</tbody></table><p class="muted" style="margin-top:8px">'+ops.length+' of '+INTRINSICS_CATALOG.reduce((s,c)=>s+c.ops.length,0)+' intrinsics</p>';
document.querySelectorAll('[data-intrinsic]').forEach(row=>row.addEventListener('click',()=>showIntrinsic(row.dataset.intrinsic)));
}
function showIntrinsic(name){
document.querySelectorAll('[data-intrinsic]').forEach(r=>r.classList.toggle('active',r.dataset.intrinsic===name));
let op,cat;
INTRINSICS_CATALOG.forEach(c=>c.ops.forEach(o=>{if(o.name===name){op=o;cat=c.cat}}));
if(!op){$('intrinsic-detail').innerHTML='<p class="muted">Not found.</p>';return;}
const argNames=Array.from({length:op.arity},(_,i)=>'arg'+(i+1));
const example={kind:'intrinsic',version:1,op:op.name,args:argNames.map(a=>({kind:'parameter',name:a}))};
$('intrinsic-detail').innerHTML='<article class="card narrative"><h3><code>'+esc(op.name)+'</code></h3><dl><dt>Category</dt><dd>'+esc(cat)+'</dd><dt>Arguments</dt><dd>'+op.arity+'</dd><dt>Description</dt><dd>'+esc(op.desc)+'</dd></dl><h4 style="margin-top:16px">Usage example</h4><pre style="padding:12px;background:#101114;border-radius:8px;border:1px solid #2b2e38;font-size:13px;line-height:1.5">'+esc(JSON.stringify(example,null,2))+'</pre></article>';
}
$('intrinsic-search').addEventListener('input',renderList);
$('intrinsic-category').addEventListener('change',renderList);
renderList();
})();
$('refresh')?.addEventListener('click',refresh);$('episode-filter')?.addEventListener('click',refreshEpisodes);refresh();
</script></body></html>`;

function start(): void {
  const port = Number.parseInt(process.env.SPOON_INSPECTOR_PORT ?? "4317", 10);
  const transport = StdioTransport.spawn(
    process.env.SPOON_SERVER ??
      fileURLToPath(
        new URL("../../../target/debug/spoon-server", import.meta.url),
      ),
  );
  const client = new SpoonClient(transport, {
    adminToken: process.env.SPOON_ADMIN_TOKEN,
  });
  const server = createInspectorServer(client);
  server.listen(port, "127.0.0.1", () =>
    console.log(`Spoon Inspector listening at http://127.0.0.1:${port}`),
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
