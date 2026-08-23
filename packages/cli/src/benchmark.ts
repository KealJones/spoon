import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { spawn } from "node:child_process";
import os from "node:os";
import path from "node:path";

import {
  SpoonClient,
  StdioTransport,
  type FalsificationMeasurementInput,
  type JsonValue,
  type Section38TelemetrySnapshot,
} from "@spoon/sdk";

import {
  createConfiguredJudge,
  type JudgeClient,
  type JudgeEvidence,
  type JudgeResult,
} from "./judge.js";

type TeacherMode = "on" | "off";
type StepKind =
  "acquisition" | "setup" | "retention" | "variant" | "conversation";

export type BenchmarkOutcomeType =
  | "answer"
  | "clarify"
  | "abstain"
  | "action"
  | "state_change"
  | "memory_recall"
  | "reconciliation"
  | "attribution"
  | "transfer"
  | "conversation"
  | "conversation_quality";

export interface BenchmarkExpectedOutcome {
  type: BenchmarkOutcomeType;
  criteria: string[];
  value?: JsonValue;
}

interface BenchmarkStep {
  prompt: string;
  expectedAnswer?: JsonValue;
  expectedOutcome?: BenchmarkExpectedOutcome;
  phaseId?: string;
  teacherMode?: TeacherMode;
}

export interface BenchmarkTurn extends BenchmarkStep {
  id: string;
  order: number;
  teacherMode?: TeacherMode;
}

export interface BenchmarkConversation {
  turns: BenchmarkTurn[];
}

interface BenchmarkVariant extends BenchmarkStep {
  id: string;
  family?: string;
  cohort?: "training" | "heldOut";
  runAfterRestart?: boolean;
}

interface BenchmarkProbe {
  id: string;
  title?: string;
  suite?: string;
  domain: string;
  family: string;
  claimTested?: string;
  setup?: JsonValue;
  acquisition: BenchmarkStep;
  additionalAcquisition: BenchmarkVariant[];
  retention: BenchmarkStep;
  variants: BenchmarkVariant[];
  conversation?: BenchmarkConversation;
}

interface BenchmarkFixture {
  version: 1;
  name: string;
  probes: BenchmarkProbe[];
}

interface BenchmarkCatalogSuite {
  id: string;
  title: string;
  purpose: string;
  fixtures: string[];
}

interface BenchmarkCatalog {
  schemaVersion: 1;
  id: string;
  title: string;
  purpose: string;
  source?: string;
  suites: BenchmarkCatalogSuite[];
}

interface PublicAskResult {
  disposition: string;
  answer: JsonValue | null;
  episode: Record<string, unknown>;
}

export interface BenchmarkStepReport {
  kind: StepKind;
  id: string;
  prompt: string;
  teacherMode: TeacherMode;
  processRestarted: boolean;
  sessionId?: string;
  expectedAnswer: JsonValue | null;
  expectedOutcome?: BenchmarkExpectedOutcome;
  actualAnswer: JsonValue | null;
  answerMatches: boolean;
  actualOutcomeType: BenchmarkOutcomeType | "unknown";
  automatedOutcomeMatches: boolean;
  outcomeMatches: boolean;
  judge: BenchmarkJudgeReport;
  disposition: string;
  episodeId: string | null;
  action: string | null;
  teacherUsed: boolean;
  teacherCalls: number;
  learnedReusableProcedure: boolean;
  rung: string;
  steps: number;
  candidates: number;
  traceSteps: number;
  cost: number;
  confidence: number | null;
  clarified: boolean;
  groundingTier: "none" | "teacher" | "soft" | "strong";
  telemetryMeasurementId: string | null;
  telemetryError?: string;
  status: "passed" | "failed" | "skipped" | "error";
  error?: string;
}

export interface BenchmarkJudgeReport {
  status: "passed" | "failed" | "inconclusive" | "disabled" | "error";
  summary?: string;
  criteria?: Array<{
    criterion: string;
    status: "met" | "not_met" | "inconclusive";
    rationale: string;
  }>;
  provider?: string;
  model?: string;
  source?: string;
  requestId?: string;
  generatedAt?: string;
  error?: string;
}

export interface BenchmarkProbeReport {
  id: string;
  title?: string;
  suite?: string;
  domain: string;
  family: string;
  claimTested?: string;
  acquisition: BenchmarkStepReport;
  additionalAcquisition: BenchmarkStepReport[];
  retention: BenchmarkStepReport;
  variants: BenchmarkStepReport[];
  conversation?: BenchmarkConversationReport;
  overall:
    | "passed"
    | "acquisition_failed"
    | "retention_failed"
    | "generalization_failed";
}

export interface BenchmarkConversationReport {
  acquisition: BenchmarkStepReport[];
  retention: BenchmarkStepReport[];
  overall: "passed" | "acquisition_failed" | "retention_failed";
}

export interface BenchmarkReport {
  version: 1;
  fixture: string;
  generatedAt: string;
  telemetryRunId: string | null;
  telemetryRunIds?: string[];
  telemetryError?: string;
  telemetrySnapshot: Section38TelemetrySnapshot | null;
  catalog?: { id: string; title: string; suites: string[] };
  probes: BenchmarkProbeReport[];
  summary: {
    total: number;
    passed: number;
    acquisitionFailures: number;
    retentionFailures: number;
    generalizationFailures: number;
    variantsSkipped: number;
  };
}

interface TelemetryContext {
  client: SpoonClient;
  runId: string | null;
  initError?: string;
}

export async function runBenchmark(
  fixturePath: string,
  reportPath = "benchmark-report.json",
): Promise<BenchmarkReport> {
  const absoluteInputPath = path.resolve(fixturePath);
  const input = JSON.parse(
    await readFile(absoluteInputPath, "utf8"),
  ) as unknown;
  const report = isCatalog(input)
    ? await runCatalog(absoluteInputPath, parseCatalog(input))
    : await runFixture(absoluteInputPath, parseFixture(input));
  return writeBenchmarkReport(report, reportPath);
}

async function runFixture(
  fixturePath: string,
  fixture: BenchmarkFixture,
  environment: NodeJS.ProcessEnv = process.env,
): Promise<BenchmarkReport> {
  const telemetry = await openTelemetry(fixture.name, environment);
  const judge = createConfiguredJudge(environment);
  const probes: BenchmarkProbeReport[] = [];

  let telemetrySnapshot: Section38TelemetrySnapshot | null = null;
  let telemetryError: string | undefined = telemetry.initError;
  try {
    for (const probe of fixture.probes) {
      probes.push(await runProbe(probe, telemetry, undefined, environment));
    }
    await judgeFixture(probes, judge);
    if (telemetry.runId !== null) {
      try {
        telemetrySnapshot = (await telemetry.client.metricsSnapshot())
          .section38;
      } catch (error) {
        telemetryError = errorMessage(error);
      }
    }
  } finally {
    telemetry.client.close();
  }

  const report: BenchmarkReport = {
    version: 1,
    fixture: path.resolve(fixturePath),
    generatedAt: new Date().toISOString(),
    telemetryRunId: telemetry.runId,
    ...(telemetryError === undefined ? {} : { telemetryError }),
    telemetrySnapshot,
    probes,
    summary: {
      total: probes.length,
      passed: probes.filter((probe) => probe.overall === "passed").length,
      acquisitionFailures: probes.filter(
        (probe) => probe.overall === "acquisition_failed",
      ).length,
      retentionFailures: probes.filter(
        (probe) => probe.overall === "retention_failed",
      ).length,
      generalizationFailures: probes.filter(
        (probe) => probe.overall === "generalization_failed",
      ).length,
      variantsSkipped: probes.reduce(
        (count, probe) =>
          count +
          probe.variants.filter((variant) => variant.status === "skipped")
            .length,
        0,
      ),
    },
  };
  return report;
}

async function runCatalog(
  catalogPath: string,
  catalog: BenchmarkCatalog,
): Promise<BenchmarkReport> {
  const fixturePaths = catalogFixturePaths(catalogPath, catalog);
  const reports: BenchmarkReport[] = [];
  for (const fixturePath of fixturePaths) {
    const fixture = parseFixture(
      JSON.parse(await readFile(fixturePath, "utf8")) as unknown,
    );
    reports.push(
      await runFixture(
        fixturePath,
        fixture,
        await isolatedFixtureEnvironment(fixture.name),
      ),
    );
  }
  const probes = reports.flatMap((report) => report.probes);
  return {
    version: 1,
    fixture: catalogPath,
    generatedAt: new Date().toISOString(),
    telemetryRunId: null,
    telemetryRunIds: reports.flatMap((report) =>
      report.telemetryRunId === null ? [] : [report.telemetryRunId],
    ),
    ...(reports.some((report) => report.telemetryError)
      ? {
          telemetryError: reports
            .filter((report) => report.telemetryError)
            .map((report) => report.telemetryError)
            .join("; "),
        }
      : {}),
    telemetrySnapshot: null,
    catalog: {
      id: catalog.id,
      title: catalog.title,
      suites: catalog.suites.map((suite) => suite.id),
    },
    probes,
    summary: summarizeReports(probes),
  };
}

async function isolatedFixtureEnvironment(
  fixtureId: string,
): Promise<NodeJS.ProcessEnv> {
  const directory = await mkdtemp(
    path.join(os.tmpdir(), `spoon-benchmark-${fixtureId.toLowerCase()}-`),
  );
  return {
    ...process.env,
    SPOON_DB: path.join(directory, "spoon.sqlite"),
  };
}

async function writeBenchmarkReport(
  report: BenchmarkReport,
  reportPath: string,
): Promise<BenchmarkReport> {
  const resolvedReportPath = path.resolve(reportPath);
  await mkdir(path.dirname(resolvedReportPath), { recursive: true });
  await writeFile(
    resolvedReportPath,
    `${JSON.stringify(report, null, 2)}\n`,
    "utf8",
  );
  await writeFile(
    markdownPath(resolvedReportPath),
    `${renderBenchmarkReport(report)}\n`,
    "utf8",
  );
  return report;
}

function summarizeReports(
  probes: BenchmarkProbeReport[],
): BenchmarkReport["summary"] {
  return {
    total: probes.length,
    passed: probes.filter((probe) => probe.overall === "passed").length,
    acquisitionFailures: probes.filter(
      (probe) => probe.overall === "acquisition_failed",
    ).length,
    retentionFailures: probes.filter(
      (probe) => probe.overall === "retention_failed",
    ).length,
    generalizationFailures: probes.filter(
      (probe) => probe.overall === "generalization_failed",
    ).length,
    variantsSkipped: probes.reduce(
      (count, probe) =>
        count +
        probe.variants.filter((variant) => variant.status === "skipped").length,
      0,
    ),
  };
}

export async function loadBenchmarkReport(
  reportPath: string,
): Promise<BenchmarkReport> {
  return JSON.parse(
    await readFile(path.resolve(reportPath), "utf8"),
  ) as BenchmarkReport;
}

export function renderBenchmarkReport(report: BenchmarkReport): string {
  const lines = [
    `${report.catalog === undefined ? "Benchmark" : "Catalog"}: ${path.basename(report.fixture)}`,
    `Result: ${report.summary.passed}/${report.summary.total} probes passed`,
    `Acquisition failures: ${report.summary.acquisitionFailures}`,
    `Exact retention failures: ${report.summary.retentionFailures}`,
    `Generalization failures: ${report.summary.generalizationFailures}`,
    `Variants skipped after retention failure: ${report.summary.variantsSkipped}`,
  ];
  if (report.telemetryRunId !== null)
    lines.push(`Telemetry run: ${report.telemetryRunId}`);
  if (report.telemetryRunIds !== undefined && report.telemetryRunIds.length > 0)
    lines.push(`Telemetry runs: ${report.telemetryRunIds.join(", ")}`);
  if (report.telemetryError) lines.push(`Telemetry: ${report.telemetryError}`);

  for (const probe of report.probes) {
    lines.push(`\n${probe.id}: ${probe.overall}`);
    lines.push(formatStep(probe.acquisition));
    for (const step of probe.additionalAcquisition)
      lines.push(formatStep(step));
    lines.push(formatStep(probe.retention));
    for (const variant of probe.variants) lines.push(formatStep(variant));
    if (probe.conversation) {
      lines.push(`  conversation: ${probe.conversation.overall}`);
      for (const turn of probe.conversation.acquisition)
        lines.push(formatStep(turn));
      for (const turn of probe.conversation.retention)
        lines.push(formatStep(turn));
    }
  }
  return lines.join("\n");
}

async function runProbe(
  probe: BenchmarkProbe,
  telemetry: TelemetryContext,
  judge: JudgeClient | undefined,
  environment: NodeJS.ProcessEnv,
): Promise<BenchmarkProbeReport> {
  const acquisition = await runStep(
    probe,
    "acquisition",
    probe.id,
    probe.acquisition,
    probe.acquisition.teacherMode ?? "on",
    "training",
    probe.family,
    telemetry,
    undefined,
    undefined,
    judge,
    environment,
  );
  const additionalAcquisition: BenchmarkStepReport[] = [];
  if (acquisition.status !== "error") {
    for (const step of probe.additionalAcquisition) {
      additionalAcquisition.push(
        await runStep(
          probe,
          "setup",
          step.id,
          step,
          "on",
          "training",
          probe.family,
          telemetry,
          acquisition.telemetryMeasurementId,
          undefined,
          judge,
          environment,
        ),
      );
    }
  }
  const retention =
    acquisition.status === "error"
      ? skippedStep(
          "retention",
          `${probe.id}:retention`,
          probe.retention,
          "acquisition could not be executed",
        )
      : await runStep(
          probe,
          "retention",
          `${probe.id}:retention`,
          probe.retention,
          probe.retention.teacherMode ?? "off",
          "training",
          probe.family,
          telemetry,
          acquisition.telemetryMeasurementId,
          undefined,
          judge,
          environment,
        );

  const variants = [] as BenchmarkStepReport[];
  for (const variant of probe.variants) {
    if (retention.status !== "passed") {
      variants.push(
        skippedStep(
          "variant",
          variant.id,
          variant,
          "exact Teacher-OFF retention did not pass",
        ),
      );
      continue;
    }
    variants.push(
      await runStep(
        probe,
        "variant",
        variant.id,
        variant,
        variant.teacherMode ?? "off",
        variant.cohort ?? "heldOut",
        variant.family ?? `${probe.family}-variant`,
        telemetry,
        undefined,
        undefined,
        judge,
        environment,
      ),
    );
  }

  const overall =
    acquisition.status !== "passed" ||
    additionalAcquisition.some((step) => step.status !== "passed")
      ? "acquisition_failed"
      : retention.status !== "passed"
        ? "retention_failed"
        : variants.some(
              (variant) =>
                variant.status !== "passed" && variant.status !== "skipped",
            )
          ? "generalization_failed"
          : "passed";
  const conversation = probe.conversation
    ? await runConversation(
        probe,
        probe.conversation,
        telemetry,
        judge,
        environment,
      )
    : undefined;
  return {
    id: probe.id,
    ...(probe.title === undefined ? {} : { title: probe.title }),
    ...(probe.suite === undefined ? {} : { suite: probe.suite }),
    domain: probe.domain,
    family: probe.family,
    ...(probe.claimTested === undefined
      ? {}
      : { claimTested: probe.claimTested }),
    acquisition,
    additionalAcquisition,
    retention,
    variants,
    ...(conversation === undefined ? {} : { conversation }),
    overall,
  };
}

async function runConversation(
  probe: BenchmarkProbe,
  conversation: BenchmarkConversation,
  telemetry: TelemetryContext,
  judge: JudgeClient | undefined,
  environment: NodeJS.ProcessEnv,
): Promise<BenchmarkConversationReport> {
  const acquisitionSession = await startPublicSession(
    `benchmark-${probe.id}-on-${Date.now()}`,
    environment,
  );
  const acquisition: BenchmarkStepReport[] = [];
  for (const turn of conversation.turns) {
    acquisition.push(
      await runConversationTurn(
        probe,
        turn,
        "acquisition",
        turn.teacherMode ?? "on",
        acquisitionSession,
        telemetry,
        judge,
        environment,
      ),
    );
  }
  if (!acquisition.every((turn) => turn.status === "passed")) {
    return {
      acquisition,
      retention: conversation.turns.map((turn) =>
        skippedStep(
          "conversation",
          `${probe.id}:conversation:retention:${turn.id}`,
          turn,
          "conversation acquisition did not pass",
        ),
      ),
      overall: "acquisition_failed",
    };
  }
  const retentionSession = await startPublicSession(
    `benchmark-${probe.id}-off-${Date.now()}`,
    environment,
  );
  const retention: BenchmarkStepReport[] = [];
  for (const turn of conversation.turns) {
    retention.push(
      await runConversationTurn(
        probe,
        turn,
        "retention",
        "off",
        retentionSession,
        telemetry,
        judge,
        environment,
      ),
    );
  }
  return {
    acquisition,
    retention,
    overall: retention.every((turn) => turn.status === "passed")
      ? "passed"
      : "retention_failed",
  };
}

async function runConversationTurn(
  probe: BenchmarkProbe,
  turn: BenchmarkTurn,
  phase: "acquisition" | "retention",
  teacherMode: TeacherMode,
  sessionId: string,
  telemetry: TelemetryContext,
  judge: JudgeClient | undefined,
  environment: NodeJS.ProcessEnv,
): Promise<BenchmarkStepReport> {
  return runStep(
    probe,
    "conversation",
    `${probe.id}:conversation:${phase}:${turn.id}`,
    turn,
    teacherMode,
    "training",
    probe.family,
    telemetry,
    undefined,
    sessionId,
    judge,
    environment,
  );
}

async function runStep(
  probe: BenchmarkProbe,
  kind: StepKind,
  id: string,
  step: BenchmarkStep,
  teacherMode: TeacherMode,
  cohort: "training" | "heldOut",
  family: string,
  telemetry: TelemetryContext,
  repeatOf?: string | null,
  sessionId?: string,
  judge?: JudgeClient,
  environment: NodeJS.ProcessEnv = process.env,
): Promise<BenchmarkStepReport> {
  try {
    const result = await runPublicAsk(
      step.prompt,
      teacherMode,
      sessionId,
      environment,
    );
    const report = summarizeStep(kind, id, step, teacherMode, result);
    if (sessionId !== undefined) report.sessionId = sessionId;
    await judgeStep(probe, step, report, judge);
    await recordTelemetry(probe, report, cohort, family, telemetry, repeatOf);
    return report;
  } catch (error) {
    return {
      ...emptyStep(kind, id, step, teacherMode),
      status: "error",
      error: errorMessage(error),
    };
  }
}

function summarizeStep(
  kind: StepKind,
  id: string,
  step: BenchmarkStep,
  teacherMode: TeacherMode,
  result: PublicAskResult,
): BenchmarkStepReport {
  const episode = result.episode;
  const action = stringValue(episode.action);
  const interaction = recordValue(
    episode.teacher_interaction ?? episode.teacherInteraction,
  );
  const teacherCalls = countTeacherProposals(interaction);
  const teacherUsed = teacherMode === "on" && teacherCalls > 0;
  const expectedAnswer =
    step.expectedAnswer ?? step.expectedOutcome?.value ?? null;
  const answerMatches =
    step.expectedAnswer === undefined
      ? result.answer !== null && result.disposition !== "abstained"
      : deepEqual(result.answer, step.expectedAnswer);
  const clarified = action?.toLowerCase().includes("clarif") ?? false;
  const actualOutcomeType = inferOutcomeType(result, action, clarified);
  const outcomeMatches =
    step.expectedOutcome === undefined
      ? answerMatches
      : matchesExpectedOutcome(
          step.expectedOutcome,
          actualOutcomeType,
          answerMatches,
          result,
          action,
          clarified,
        );
  return {
    kind,
    id,
    prompt: step.prompt,
    teacherMode,
    processRestarted: true,
    expectedAnswer,
    ...(step.expectedOutcome === undefined
      ? {}
      : { expectedOutcome: step.expectedOutcome }),
    actualAnswer: result.answer,
    answerMatches,
    actualOutcomeType,
    automatedOutcomeMatches: outcomeMatches,
    outcomeMatches,
    judge: { status: "disabled" },
    disposition: result.disposition,
    episodeId: stringValue(episode.id) ?? null,
    action,
    teacherUsed,
    teacherCalls,
    learnedReusableProcedure:
      teacherUsed && action !== null && action.startsWith("procedure:"),
    rung:
      stringValue(
        recordValue(episode.cost)?.rung_reached ??
          recordValue(episode.cost)?.rungReached,
      ) ?? "unknown",
    steps: numberValue(
      recordValue(episode.cost)?.steps_taken ??
        recordValue(episode.cost)?.stepsTaken,
    ),
    candidates: Array.isArray(episode.knowledge_considered)
      ? episode.knowledge_considered.length
      : Array.isArray(episode.knowledgeConsidered)
        ? episode.knowledgeConsidered.length
        : 0,
    traceSteps: traceStepCount(episode),
    cost: numberValue(
      recordValue(episode.cost)?.budget_spent ??
        recordValue(episode.cost)?.budgetSpent,
    ),
    confidence: evaluationConfidence(episode),
    clarified,
    groundingTier:
      teacherMode === "off"
        ? action?.startsWith("procedure:")
          ? "strong"
          : "none"
        : teacherUsed && action?.startsWith("procedure:")
          ? "strong"
          : teacherUsed
            ? "teacher"
            : "none",
    telemetryMeasurementId: null,
    status: outcomeMatches ? "passed" : "failed",
  };
}

async function judgeStep(
  probe: BenchmarkProbe,
  step: BenchmarkStep,
  report: BenchmarkStepReport,
  judge: JudgeClient | undefined,
): Promise<void> {
  if (judge === undefined) return;
  try {
    const result = await judge.judge({
      probeId: probe.id,
      phaseId: step.phaseId ?? report.id,
      prompt: report.prompt,
      expectedOutcome: expectedOutcomeForJudge(step),
      actualAnswer: report.actualAnswer,
      disposition: report.disposition,
      action: report.action,
      teacherMode: report.teacherMode,
      teacherCalls: report.teacherCalls,
      rung: report.rung,
      traceSteps: report.traceSteps,
      confidence: report.confidence,
      groundingTier: report.groundingTier,
    });
    applyJudgeResult(report, result);
  } catch (error) {
    report.outcomeMatches = false;
    report.status = "error";
    report.judge = { status: "error", error: errorMessage(error) };
  }
}

async function judgeFixture(
  probes: BenchmarkProbeReport[],
  judge: JudgeClient | undefined,
): Promise<void> {
  if (judge === undefined) return;
  const entries = judgeableFixtureSteps(probes);
  if (entries.length === 0) return;
  try {
    const results = await judge.judgeBatch(
      entries.map(({ id, probeId, report }) => ({
        id,
        evidence: judgeEvidence(probeId, report),
      })),
    );
    const byId = new Map(results.map((result) => [result.id, result.result]));
    for (const entry of entries) {
      const result = byId.get(entry.id);
      if (result === undefined) {
        throw new Error(`Judge batch omitted ${entry.id}`);
      }
      applyJudgeResult(entry.report, result);
    }
  } catch (error) {
    const message = errorMessage(error);
    for (const entry of entries) {
      entry.report.outcomeMatches = false;
      entry.report.status = "error";
      entry.report.judge = { status: "error", error: message };
    }
  }
}

function judgeableFixtureSteps(
  probes: BenchmarkProbeReport[],
): Array<{ id: string; probeId: string; report: BenchmarkStepReport }> {
  const entries: Array<{
    id: string;
    probeId: string;
    report: BenchmarkStepReport;
  }> = [];
  for (const probe of probes) {
    const reports = [
      probe.acquisition,
      ...probe.additionalAcquisition,
      probe.retention,
      ...probe.variants,
      ...(probe.conversation === undefined
        ? []
        : [...probe.conversation.acquisition, ...probe.conversation.retention]),
    ];
    for (const report of reports) {
      if (report.status === "skipped" || report.status === "error") continue;
      entries.push({
        id: `${probe.id}:${report.kind}:${report.id}`,
        probeId: probe.id,
        report,
      });
    }
  }
  return entries;
}

function judgeEvidence(
  probeId: string,
  report: BenchmarkStepReport,
): JudgeEvidence {
  return {
    probeId,
    phaseId: report.id,
    prompt: report.prompt,
    expectedOutcome:
      report.expectedOutcome === undefined
        ? report.expectedAnswer === null
          ? null
          : { type: "answer", value: report.expectedAnswer, criteria: [] }
        : {
            type: report.expectedOutcome.type,
            criteria: report.expectedOutcome.criteria,
            ...(report.expectedOutcome.value === undefined
              ? {}
              : { value: report.expectedOutcome.value }),
          },
    actualAnswer: report.actualAnswer,
    disposition: report.disposition,
    action: report.action,
    teacherMode: report.teacherMode,
    teacherCalls: report.teacherCalls,
    rung: report.rung,
    traceSteps: report.traceSteps,
    confidence: report.confidence,
    groundingTier: report.groundingTier,
  };
}

function expectedOutcomeForJudge(step: BenchmarkStep): JsonValue | null {
  if (step.expectedOutcome === undefined) {
    return step.expectedAnswer === undefined
      ? null
      : { type: "answer", value: step.expectedAnswer, criteria: [] };
  }
  return {
    type: step.expectedOutcome.type,
    criteria: step.expectedOutcome.criteria,
    ...(step.expectedOutcome.value === undefined
      ? {}
      : { value: step.expectedOutcome.value }),
  };
}

function applyJudgeResult(
  report: BenchmarkStepReport,
  result: JudgeResult,
): void {
  const verdictStatus =
    result.verdict.verdict === "pass"
      ? "passed"
      : result.verdict.verdict === "fail"
        ? "failed"
        : "inconclusive";
  report.judge = {
    status: verdictStatus,
    summary: result.verdict.summary,
    criteria: result.verdict.criteria,
    provider: result.provenance.provider,
    ...(result.provenance.model === undefined
      ? {}
      : { model: result.provenance.model }),
    source: result.provenance.source,
    requestId: result.provenance.requestId,
    generatedAt: result.provenance.generatedAt,
  };
  report.outcomeMatches =
    report.automatedOutcomeMatches && result.verdict.verdict === "pass";
  report.status = report.outcomeMatches ? "passed" : "failed";
}

function inferOutcomeType(
  result: PublicAskResult,
  action: string | null,
  clarified: boolean,
): BenchmarkOutcomeType | "unknown" {
  if (clarified) return "clarify";
  if (result.disposition === "abstained") return "abstain";
  const lowered = action?.toLowerCase() ?? "";
  if (lowered.includes("reconcil")) return "reconciliation";
  if (lowered.includes("attribut") || lowered.includes("analysis"))
    return "attribution";
  if (lowered.includes("recall")) return "memory_recall";
  if (lowered.startsWith("procedure:") || lowered.includes("execute"))
    return "action";
  return result.answer === null ? "unknown" : "answer";
}

function evaluationConfidence(episode: Record<string, unknown>): number | null {
  const evaluation = recordValue(episode.evaluation);
  const tier = stringValue(evaluation?.tier)?.toLowerCase();
  if (tier === "hard") return 0.9;
  if (tier === "consensus") return 0.8;
  if (tier === "deferred") return 0.3;
  return null;
}

function matchesExpectedOutcome(
  expected: BenchmarkExpectedOutcome,
  actual: BenchmarkOutcomeType | "unknown",
  answerMatches: boolean,
  result: PublicAskResult,
  action: string | null,
  clarified: boolean,
): boolean {
  if (expected.type === "answer")
    return expected.value === undefined
      ? answerMatches
      : deepEqual(result.answer, expected.value);
  if (expected.type === "clarify") return clarified;
  if (expected.type === "abstain") return result.disposition === "abstained";
  if (expected.type === "action") return actual === "action";
  if (expected.type === "state_change") return action !== null;
  if (expected.type === "memory_recall")
    return (
      actual === "memory_recall" || (result.answer !== null && action !== null)
    );
  if (expected.type === "reconciliation") return actual === "reconciliation";
  if (expected.type === "attribution") return actual === "attribution";
  if (expected.type === "transfer")
    return result.answer !== null && result.disposition !== "abstained";
  if (expected.type === "conversation")
    return result.answer !== null || action !== null;
  if (expected.type === "conversation_quality")
    return result.answer !== null || action !== null;
  return actual === expected.type;
}

async function recordTelemetry(
  probe: BenchmarkProbe,
  step: BenchmarkStepReport,
  cohort: "training" | "heldOut",
  family: string,
  telemetry: TelemetryContext,
  repeatOf?: string | null,
): Promise<void> {
  if (telemetry.runId === null) return;
  const correct =
    step.status === "passed" && !step.disposition.startsWith("abstained");
  const measurement: FalsificationMeasurementInput = {
    domain: probe.domain,
    family,
    cohort,
    probeId: step.kind === "variant" ? step.id : probe.id,
    noveltyIdentity: `${probe.id}:${stableJson(step.prompt)}`,
    repeatOf: repeatOf ?? null,
    teacherMode: step.teacherMode,
    teacherUsed: step.teacherUsed,
    teacherCalls: step.teacherCalls,
    rung: step.rung,
    steps: step.steps,
    candidates: step.candidates,
    traceSteps: step.traceSteps,
    cost: step.cost,
    abstained: step.disposition === "abstained",
    clarified: step.clarified,
    confidence: step.confidence,
    groundingTier: step.groundingTier,
    correct: step.disposition === "abstained" ? null : correct,
    failureReason:
      correct || step.disposition === "abstained"
        ? null
        : `answer did not match expected value (${step.disposition})`,
    regressionProbe: step.kind === "retention",
  };
  try {
    const recorded = await telemetry.client.recordFalsificationMeasurement(
      telemetry.runId,
      measurement,
    );
    step.telemetryMeasurementId = recorded.id;
  } catch (error) {
    step.telemetryError = errorMessage(error);
  }
}

async function openTelemetry(
  label: string,
  environment: NodeJS.ProcessEnv,
): Promise<TelemetryContext> {
  const client = new SpoonClient(
    StdioTransport.spawn(
      environment.SPOON_SERVER ?? "target/debug/spoon-server",
      [],
      { env: environment },
    ),
  );
  try {
    const run = await client.createFalsificationRun({
      label: `benchmark:${label}`,
      benchmark: label,
      notes: "Public CLI acquisition/retention/generalization run",
    });
    return { client, runId: run.id };
  } catch (error) {
    return { client, runId: null, initError: errorMessage(error) };
  }
}

async function runPublicAsk(
  prompt: string,
  teacherMode: TeacherMode,
  sessionId?: string,
  environment: NodeJS.ProcessEnv = process.env,
): Promise<PublicAskResult> {
  const args = [
    "--silent",
    "spoon",
    "ask",
    "--teacher",
    teacherMode,
    ...(sessionId === undefined ? [] : ["--session", sessionId]),
    prompt,
  ];
  const result = await runProcess("pnpm", args, environment);
  if (result.exitCode !== 0) {
    throw new Error(
      result.stderr.trim() || `ask exited with ${result.exitCode}`,
    );
  }
  const parsed = JSON.parse(result.stdout.trim()) as unknown;
  if (!isRecord(parsed) || !isRecord(parsed.episode)) {
    throw new Error("ask did not return a completed public result");
  }
  return {
    disposition: stringValue(parsed.disposition) ?? "unknown",
    answer: (parsed.answer ?? null) as JsonValue | null,
    episode: parsed.episode,
  };
}

async function startPublicSession(
  name: string,
  environment: NodeJS.ProcessEnv,
): Promise<string> {
  const result = await runProcess(
    "pnpm",
    ["--silent", "spoon", "session", "start", "--name", name],
    environment,
  );
  if (result.exitCode !== 0)
    throw new Error(
      result.stderr.trim() || `session start exited with ${result.exitCode}`,
    );
  const parsed = JSON.parse(result.stdout.trim()) as unknown;
  if (!isRecord(parsed) || typeof parsed.id !== "string")
    throw new Error("session start did not return a session id");
  return parsed.id;
}

function emptyStep(
  kind: StepKind,
  id: string,
  step: BenchmarkStep,
  teacherMode: TeacherMode,
): BenchmarkStepReport {
  return {
    kind,
    id,
    prompt: step.prompt,
    teacherMode,
    processRestarted: true,
    expectedAnswer: step.expectedAnswer ?? step.expectedOutcome?.value ?? null,
    ...(step.expectedOutcome === undefined
      ? {}
      : { expectedOutcome: step.expectedOutcome }),
    actualAnswer: null,
    answerMatches: false,
    actualOutcomeType: "unknown",
    automatedOutcomeMatches: false,
    outcomeMatches: false,
    judge: { status: "disabled" },
    disposition: "error",
    episodeId: null,
    action: null,
    teacherUsed: false,
    teacherCalls: 0,
    learnedReusableProcedure: false,
    rung: "unknown",
    steps: 0,
    candidates: 0,
    traceSteps: 0,
    cost: 0,
    confidence: null,
    clarified: false,
    groundingTier: "none",
    telemetryMeasurementId: null,
    status: "error",
  };
}

function skippedStep(
  kind: StepKind,
  id: string,
  step: BenchmarkStep,
  reason: string,
): BenchmarkStepReport {
  return {
    ...emptyStep(kind, id, step, "off"),
    status: "skipped",
    error: reason,
  };
}

function parseFixture(value: unknown): BenchmarkFixture {
  if (!isRecord(value)) throw new Error("benchmark fixture must be an object");
  if (value.schemaVersion === 1 && typeof value.id === "string") {
    return parseExperimentFixture(value);
  }
  throw new Error("benchmark input must use schemaVersion 1 experiment format");
}

function isCatalog(value: unknown): value is Record<string, unknown> {
  return isRecord(value) && Array.isArray(value.suites);
}

function parseCatalog(value: Record<string, unknown>): BenchmarkCatalog {
  if (value.schemaVersion !== 1)
    throw new Error("benchmark catalog must use schemaVersion 1");
  const id = requiredString(value.id, "catalog id");
  const title = requiredString(value.title, `${id}.title`);
  const purpose = requiredString(value.purpose, `${id}.purpose`);
  if (!Array.isArray(value.suites) || value.suites.length === 0) {
    throw new Error(`${id}.suites must contain at least one suite`);
  }
  const suites = value.suites.map((rawSuite, index) => {
    const suite = phaseRecord(rawSuite, `${id}.suites[${index}]`);
    const suiteId = requiredString(suite.id, `${id}.suites[${index}].id`);
    const suiteTitle = requiredString(
      suite.title,
      `${id}.suites[${index}].title`,
    );
    const suitePurpose = requiredString(
      suite.purpose,
      `${id}.suites[${index}].purpose`,
    );
    if (
      !Array.isArray(suite.fixtures) ||
      suite.fixtures.length === 0 ||
      !suite.fixtures.every((fixture) => typeof fixture === "string")
    ) {
      throw new Error(`${id}.${suiteId}.fixtures must contain fixture ids`);
    }
    return {
      id: suiteId,
      title: suiteTitle,
      purpose: suitePurpose,
      fixtures: suite.fixtures as string[],
    };
  });
  return {
    schemaVersion: 1,
    id,
    title,
    purpose,
    ...(value.source === undefined
      ? {}
      : { source: requiredString(value.source, `${id}.source`) }),
    suites,
  };
}

function catalogFixturePaths(
  catalogPath: string,
  catalog: BenchmarkCatalog,
): string[] {
  const base = path.dirname(catalogPath);
  const paths: string[] = [];
  const seen = new Set<string>();
  for (const suite of catalog.suites) {
    for (const fixture of suite.fixtures) {
      const candidate =
        fixture.endsWith(".json") || fixture.includes("/")
          ? path.resolve(base, fixture)
          : path.resolve(base, "fixtures", `${fixture}.json`);
      if (seen.has(candidate)) {
        throw new Error(`catalog fixture appears more than once: ${fixture}`);
      }
      seen.add(candidate);
      paths.push(candidate);
    }
  }
  return paths;
}

function parseExperimentFixture(
  value: Record<string, unknown>,
): BenchmarkFixture {
  const id = requiredString(value.id, "fixture id");
  const title =
    value.title === undefined
      ? undefined
      : requiredString(value.title, `${id}.title`);
  const suite =
    value.suite === undefined
      ? undefined
      : requiredString(value.suite, `${id}.suite`);
  const family = requiredString(value.family, `${id}.family`);
  const rawPhases = value.phases;
  const phases: Array<Record<string, unknown>> = Array.isArray(rawPhases)
    ? rawPhases.map((phase, index) =>
        phaseRecord(phase, `${id}.phases[${index}]`),
      )
    : [];
  if (phases.length === 0)
    throw new Error(`${id}.phases must contain at least one phase`);

  const acquisition = phases.find((phase) => phase.id === "acquisition");
  const retention = phases.find((phase) => phase.id === "retention");
  if (!acquisition || !retention) {
    throw new Error(
      `${id}.phases must include acquisition and retention phases`,
    );
  }
  const standardIds = new Set(["acquisition", "retention"]);
  const additionalAcquisition = phases
    .filter((phase) => String(phase.id).startsWith("teach-"))
    .map((phase, index) => {
      const phaseId = requiredString(phase.id, `${id}.phases[${index}].id`);
      const step = parseExperimentStep(phase, `${id}.${phaseId}`, phaseId);
      if (step.teacherMode !== "on") {
        throw new Error(`${id}.${phaseId}.teacher.mode must be on`);
      }
      return { ...step, id: phaseId, cohort: "training" as const };
    });
  const variants = phases
    .filter(
      (phase) =>
        !standardIds.has(String(phase.id)) &&
        !String(phase.id).startsWith("teach-"),
    )
    .map((phase, index) => {
      const phaseId = requiredString(phase.id, `${id}.phases[${index}].id`);
      const step = parseExperimentStep(phase, `${id}.${phaseId}`, phaseId);
      if (step.teacherMode !== "off") {
        throw new Error(
          `${id}.${phaseId}.teacher.mode must be off for held-out phases`,
        );
      }
      return {
        ...step,
        id: phaseId,
        cohort: "heldOut" as const,
      };
    });
  const acquisitionStep = parseExperimentStep(
    acquisition,
    `${id}.acquisition`,
    "acquisition",
  );
  const retentionStep = parseExperimentStep(
    retention,
    `${id}.retention`,
    "retention",
  );
  if (acquisitionStep.teacherMode !== "on") {
    throw new Error(`${id}.acquisition.teacher.mode must be on`);
  }
  if (retentionStep.teacherMode !== "off") {
    throw new Error(`${id}.retention.teacher.mode must be off`);
  }
  const probe: BenchmarkProbe = {
    id,
    ...(title === undefined ? {} : { title }),
    ...(suite === undefined ? {} : { suite }),
    domain: suite ?? family,
    family,
    ...(value.claimTested === undefined
      ? {}
      : {
          claimTested: requiredString(value.claimTested, `${id}.claimTested`),
        }),
    ...(value.setup === undefined
      ? {}
      : { setup: jsonValue(value.setup, `${id}.setup`) }),
    acquisition: acquisitionStep,
    additionalAcquisition,
    retention: retentionStep,
    variants,
  };
  return { version: 1, name: title ?? id, probes: [probe] };
}

function phaseRecord(value: unknown, label: string): Record<string, unknown> {
  if (!isRecord(value)) throw new Error(`${label} must be an object`);
  return value;
}

function parseExperimentStep(
  phase: Record<string, unknown>,
  label: string,
  phaseId: string,
): BenchmarkStep {
  if (
    typeof phase.order !== "number" ||
    !Number.isInteger(phase.order) ||
    phase.order < 1
  ) {
    throw new Error(`${label}.order must be a positive integer`);
  }
  requiredString(phase.purpose, `${label}.purpose`);
  if (
    !isRecord(phase.teacher) ||
    (phase.teacher.mode !== "on" && phase.teacher.mode !== "off")
  ) {
    throw new Error(`${label}.teacher.mode must be on or off`);
  }
  const input = isRecord(phase.input) ? phase.input : phase;
  const prompt = requiredString(input.prompt, `${label}.input.prompt`);
  const expectedOutcome = parseExpectedOutcome(
    phase.expectedOutcome,
    `${label}.expectedOutcome`,
  );
  const expectedAnswer = phase.expectedAnswer ?? input.expectedAnswer;
  if (expectedAnswer === undefined && expectedOutcome === undefined) {
    throw new Error(`${label} must define expectedOutcome or expectedAnswer`);
  }
  return {
    prompt,
    ...(expectedAnswer === undefined
      ? {}
      : {
          expectedAnswer: jsonValue(expectedAnswer, `${label}.expectedAnswer`),
        }),
    ...(expectedOutcome === undefined ? {} : { expectedOutcome }),
    phaseId,
    teacherMode: phase.teacher.mode,
  };
}

function parseExpectedOutcome(
  value: unknown,
  label: string,
): BenchmarkExpectedOutcome | undefined {
  if (value === undefined) return undefined;
  if (!isRecord(value)) throw new Error(`${label} must be an object`);
  const type = value.type;
  const allowed: BenchmarkOutcomeType[] = [
    "answer",
    "clarify",
    "abstain",
    "action",
    "state_change",
    "memory_recall",
    "reconciliation",
    "attribution",
    "transfer",
    "conversation",
    "conversation_quality",
  ];
  if (
    typeof type !== "string" ||
    !allowed.includes(type as BenchmarkOutcomeType)
  ) {
    throw new Error(`${label}.type is not a supported outcome type`);
  }
  const criteria =
    value.criteria === undefined
      ? []
      : Array.isArray(value.criteria)
        ? value.criteria.map((criterion, index) =>
            requiredString(criterion, `${label}.criteria[${index}]`),
          )
        : (() => {
            throw new Error(`${label}.criteria must be an array`);
          })();
  return {
    type: type as BenchmarkOutcomeType,
    criteria,
    ...(value.value === undefined
      ? {}
      : { value: jsonValue(value.value, `${label}.value`) }),
  };
}

export function parseBenchmarkFixture(value: unknown): BenchmarkFixture {
  return parseFixture(value);
}

function formatStep(step: BenchmarkStepReport): string {
  const answer = JSON.stringify(step.actualAnswer);
  const suffix = step.error ? ` — ${step.error}` : "";
  const learning = step.learnedReusableProcedure
    ? "reusable-lesson-admitted"
    : "no-reusable-lesson";
  const judge =
    step.judge.status === "disabled"
      ? "judge=disabled"
      : step.judge.status === "error"
        ? `judge=error:${step.judge.error ?? "unknown"}`
        : `judge=${step.judge.status}:${step.judge.provider ?? "unknown"}`;
  return `  ${step.kind} [${step.teacherMode}] ${step.status}: ${step.prompt} → ${answer} (teacher=${step.teacherUsed ? "yes" : "no"}, calls=${step.teacherCalls}, ${judge}, learning=${learning}, rung=${step.rung}, cost=${step.cost}, episode=${step.episodeId ?? "none"})${suffix}`;
}

function markdownPath(reportPath: string): string {
  return reportPath.replace(/\.json$/i, ".md");
}

function requiredString(value: unknown, label: string): string {
  if (typeof value !== "string" || value.trim() === "")
    throw new Error(`${label} must be a non-empty string`);
  return value;
}

function jsonValue(value: unknown, label: string): JsonValue {
  if (!isJsonValue(value)) throw new Error(`${label} must be JSON-compatible`);
  return value;
}

function isJsonValue(value: unknown): value is JsonValue {
  if (value === null || typeof value === "string" || typeof value === "boolean")
    return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.every(isJsonValue);
  return isRecord(value) && Object.values(value).every(isJsonValue);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function recordValue(value: unknown): Record<string, unknown> | null {
  return isRecord(value) ? value : null;
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function numberValue(value: unknown): number {
  return typeof value === "number" && Number.isFinite(value) ? value : 0;
}

function traceStepCount(episode: Record<string, unknown>): number {
  const trace = recordValue(episode.reasoning_trace ?? episode.reasoningTrace);
  const steps = trace?.steps;
  return Array.isArray(steps) ? steps.length : 0;
}

function countTeacherProposals(value: unknown): number {
  if (Array.isArray(value))
    return value.reduce<number>(
      (sum, item) => sum + countTeacherProposals(item),
      0,
    );
  if (!isRecord(value)) return 0;
  const own = Object.prototype.hasOwnProperty.call(value, "proposal") ? 1 : 0;
  return (
    own +
    Object.values(value).reduce<number>(
      (sum, item) => sum + countTeacherProposals(item),
      0,
    )
  );
}

function deepEqual(left: unknown, right: unknown): boolean {
  return stableJson(left) === stableJson(right);
}

function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(",")}]`;
  if (isRecord(value)) {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function runProcess(
  command: string,
  args: string[],
  environment: NodeJS.ProcessEnv = process.env,
): Promise<{ stdout: string; stderr: string; exitCode: number }> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: process.cwd(),
      env: environment,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk: Buffer | string) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk: Buffer | string) => {
      stderr += chunk.toString();
    });
    child.once("error", reject);
    child.once("close", (exitCode) => {
      resolve({ stdout, stderr, exitCode: exitCode ?? 1 });
    });
  });
}
