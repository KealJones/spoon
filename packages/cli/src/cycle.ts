import {
  JsonRpcError,
  SpoonClient,
  type CompletedCycleProgress,
  type TeacherRequestWire,
} from "@spoon/sdk";
import {
  ClaudeTeacher,
  CodexTeacher,
  CursorTeacher,
  HumanTeacher,
  OllamaTeacher,
  OpenAITeacher,
  type KnowledgeContext,
  type PromptBuilder,
  type ProposalSchema,
  type Teacher,
  type TeacherRequest,
} from "@spoon/teacher";
import {
  CursorLanguageInterpreter,
  OllamaLanguageInterpreter,
  type LanguageInterpreter,
  type IntentProposalWire,
} from "@spoon/intent";

export type TeacherClient = Pick<Teacher, "propose" | "validationPipeline">;

/** Shared provider transport options for a structured-output protocol. */
export interface ProviderProtocolOptions {
  systemPrompt?: string;
  promptBuilder?: PromptBuilder;
}

export interface CycleRunOptions {
  maxExecSteps?: number;
  maxContextItems?: number;
  maxTeacherTurns?: number;
  sessionId?: string;
  recallMode?: "global" | "session" | "none";
  permissionMode?: "ask" | "workspace" | "full-access" | "god-mode";
}

export async function runCycle(
  client: SpoonClient,
  situation: string,
  teacher?: TeacherClient,
  options: CycleRunOptions = {},
  interpreter?: LanguageInterpreter,
): Promise<CompletedCycleProgress> {
  const maxTeacherTurns = options.maxTeacherTurns ?? 2;
  let progress = await client.beginCycle({
    situation,
    workingDirectory: process.cwd(),
    environment: {},
    assumptions: [],
    budget: {
      maxExecSteps: options.maxExecSteps ?? 10_000,
      maxContextItems: options.maxContextItems ?? 64,
      maxTeacherTurns,
    },
    teacherAllowed: teacher !== undefined,
    interpreterAllowed: interpreter !== undefined,
    ...(options.sessionId === undefined
      ? {}
      : { sessionId: options.sessionId }),
    ...(options.recallMode === undefined
      ? {}
      : { recallMode: options.recallMode }),
    ...(options.permissionMode === undefined
      ? {}
      : { permissionMode: options.permissionMode }),
  });

  let teacherTurns = 0;
  let activeCycleId: string | undefined;
  while (progress.status !== "completed") {
    if (progress.status === "need_intent") {
      if (interpreter === undefined) {
        progress = await client.skipIntent(
          progress.cycleId,
          "language interpreter is disabled for this run",
        );
        continue;
      }
      let proposal: IntentProposalWire;
      try {
        proposal = await interpreter.interpret(progress.request);
      } catch (error) {
        // No proposal reached the engine, so the cycle is still pending and
        // skipping it records an honest interpreter-failure abstention.
        progress = await client.skipIntent(
          progress.cycleId,
          describeCycleError(error),
        );
        continue;
      }
      try {
        progress = await client.resumeIntent(progress.cycleId, proposal);
      } catch (error) {
        // The engine finalizes a cycle whose resume failed, so skipping it
        // afterwards only replaces the real rejection with an ownership
        // complaint about a cycle that no longer exists. Report the rejection
        // and the proposal that caused it instead.
        throw new Error(
          `the interpreter proposal was rejected: ${describeCycleError(error)}\n` +
            `proposal: ${JSON.stringify(proposal.content)}`,
          { cause: error },
        );
      }
      continue;
    }
    if (teacher === undefined) {
      progress = await client.abortCycle(
        progress.cycleId,
        "teacher is disabled for this run",
      );
      break;
    }
    if (activeCycleId !== undefined && activeCycleId !== progress.cycleId) {
      progress = await client.abortCycle(
        progress.cycleId,
        "teacher continuation changed cycle identity",
      );
      break;
    }
    activeCycleId = progress.cycleId;
    if (teacherTurns >= maxTeacherTurns) {
      progress = await client.abortCycle(
        progress.cycleId,
        "teacher continuation exceeded the configured turn budget",
      );
      break;
    }
    teacherTurns += 1;
    const request = toTeacherRequest(progress.request);
    try {
      const proposal = await teacher.propose(request);
      const validated = await teacher
        .validationPipeline()
        .validate(proposal, request);
      progress = await client.resumeCycle(progress.cycleId, {
        ...proposal,
        validation: {
          status: validated.status,
          ...validated.validation,
        },
      });
    } catch (error) {
      progress = await client.abortCycle(
        progress.cycleId,
        error instanceof Error ? error.message : String(error),
      );
    }
  }

  if (progress.status !== "completed") {
    throw new Error("The teacher turn budget was exhausted before completion");
  }
  return progress;
}

/**
 * Explicit authoring mode. This is deliberately separate from `ask`: the
 * user has opted into durable knowledge creation, while the Engine still
 * owns lesson compilation, validation, and persistence.
 */
export async function runTeaching(
  client: SpoonClient,
  instruction: string,
  teacher: TeacherClient,
  options: CycleRunOptions = {},
  forceHeuristic = false,
): Promise<CompletedCycleProgress> {
  const situation = [
    "The user explicitly requested that Spoon teach and install a reusable procedure.",
    "Author a safe, generalizable pure_expr_v2 reusable lesson for the request below. If an advertised imported capability is required, use an explicit capability_call in the procedure body; permission is checked at execution time.",
    "Do not answer only when the request is a deterministic transformation that can be represented as a procedure.",
    "The Engine will validate and persist the lesson; never invent capability ids, effects, or unsupported operations.",
    ...(forceHeuristic
      ? [
          "The user explicitly authorizes a best-effort heuristic procedure. Do not abstain merely because an external source is variable, a result is provisional, or the heuristic cannot prove a fact. Encode the strongest bounded procedure the available IR permits, label its output and concepts as heuristic/provisional, and leave permission and live execution to runtime.",
          "When the request explicitly describes a capability operation, the selected invocation procedure itself must contain that capability_call or compose a sibling procedure that does. Do not install only a parsing helper, a constant, or a partial sub-step in place of the requested end-to-end procedure.",
        ]
      : []),
    "Teaching request:",
    instruction,
  ].join("\n");
  const result = await runCycle(client, situation, teacher, options);
  if (result.procedureIr === undefined) {
    throw new Error(
      "The Teacher did not produce an installable reusable procedure; nothing was added.",
    );
  }
  return result;
}

function describeCycleError(error: unknown): string {
  if (error instanceof JsonRpcError && isRecord(error.data)) {
    const cause = error.data.cause;
    if (typeof cause === "string" && cause.length > 0) {
      return `${error.message}: ${cause}`;
    }
  }
  return error instanceof Error ? error.message : String(error);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function createConfiguredTeacher(
  environment: NodeJS.ProcessEnv = process.env,
  protocol: ProviderProtocolOptions = {},
): TeacherClient {
  const provider = environment.SPOON_TEACHER?.toLowerCase() ?? "claude";
  const model =
    environment.SPOON_TEACHER_MODEL ??
    (provider === "ollama" ? environment.SPOON_INTERPRETER_MODEL : undefined);
  switch (provider) {
    case "claude":
      return new ClaudeTeacher({ model, ...protocol });
    case "codex":
      return new CodexTeacher({
        model,
        command: environment.SPOON_CODEX_COMMAND,
        ...protocol,
      });
    case "cursor":
      return new CursorTeacher({
        model,
        command: environment.SPOON_CURSOR_COMMAND,
        ...protocol,
      });
    case "ollama":
      return new OllamaTeacher({
        model,
        baseUrl: environment.SPOON_OLLAMA_URL,
        ...protocol,
      });
    case "human":
      return new HumanTeacher({ promptBuilder: protocol.promptBuilder });
    case "openai":
      if (!model) {
        throw new Error(
          "SPOON_TEACHER_MODEL is required for the OpenAI teacher",
        );
      }
      return new OpenAITeacher({ model, ...protocol });
    default:
      throw new Error(
        `Unknown Spoon teacher '${provider}'; expected claude, codex, cursor, openai, ollama, or human (configured with SPOON_TEACHER)`,
      );
  }
}

export function createConfiguredInterpreter(
  environment: NodeJS.ProcessEnv = process.env,
): LanguageInterpreter | undefined {
  const provider = environment.SPOON_INTERPRETER?.trim().toLowerCase();
  if (provider === undefined || provider === "" || provider === "off") {
    return undefined;
  }
  if (provider === "cursor") {
    return new CursorLanguageInterpreter({
      model: environment.SPOON_INTERPRETER_MODEL,
      command: environment.SPOON_CURSOR_COMMAND,
    });
  }
  if (provider !== "ollama") {
    throw new Error(
      `Unknown Spoon language interpreter '${provider}'; expected ollama, cursor, or off`,
    );
  }
  return new OllamaLanguageInterpreter({
    model: environment.SPOON_INTERPRETER_MODEL,
    baseUrl: environment.SPOON_OLLAMA_URL,
  });
}

function toTeacherRequest(request: TeacherRequestWire): TeacherRequest {
  const result: TeacherRequest = {
    situation: request.situation,
    context: request.context as KnowledgeContext,
    desiredOutput: request.desiredOutput as ProposalSchema,
  };
  if (request.specificQuestion !== undefined) {
    result.specificQuestion = request.specificQuestion;
  }
  return result;
}
