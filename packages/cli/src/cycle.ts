import {
  SpoonClient,
  type CompletedCycleProgress,
  type TeacherRequestWire,
} from "@spoon/sdk";
import {
  ClaudeTeacher,
  CodexTeacher,
  HumanTeacher,
  OllamaTeacher,
  OpenAITeacher,
  type KnowledgeContext,
  type ProposalSchema,
  type Teacher,
  type TeacherRequest,
} from "@spoon/teacher";

export type TeacherClient = Pick<Teacher, "propose" | "validationPipeline">;

export interface CycleRunOptions {
  maxExecSteps?: number;
  maxContextItems?: number;
  maxTeacherTurns?: number;
}

export async function runCycle(
  client: SpoonClient,
  situation: string,
  teacher?: TeacherClient,
  options: CycleRunOptions = {},
): Promise<CompletedCycleProgress> {
  const maxTeacherTurns = options.maxTeacherTurns ?? 2;
  let progress = await client.beginCycle({
    situation,
    environment: {},
    assumptions: [],
    budget: {
      maxExecSteps: options.maxExecSteps ?? 10_000,
      maxContextItems: options.maxContextItems ?? 64,
      maxTeacherTurns,
    },
    teacherAllowed: teacher !== undefined,
  });

  let teacherTurns = 0;
  let activeCycleId: string | undefined;
  while (progress.status === "need_teacher") {
    if (teacher === undefined) {
      throw new Error("The engine requested a teacher, but none is configured");
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

export function createConfiguredTeacher(
  environment: NodeJS.ProcessEnv = process.env,
): TeacherClient {
  const provider = environment.SPOON_TEACHER?.toLowerCase() ?? "claude";
  const model = environment.SPOON_TEACHER_MODEL;
  switch (provider) {
    case "claude":
      return new ClaudeTeacher({ model });
    case "codex":
      return new CodexTeacher({
        model,
        command: environment.SPOON_CODEX_COMMAND,
      });
    case "ollama":
      return new OllamaTeacher({ model });
    case "human":
      return new HumanTeacher();
    case "openai":
      if (!model) {
        throw new Error(
          "SPOON_TEACHER_MODEL is required for the OpenAI teacher",
        );
      }
      return new OpenAITeacher({ model });
    default:
      throw new Error(
        `Unknown Spoon teacher '${provider}'; expected claude, codex, openai, ollama, or human (configured with SPOON_TEACHER)`,
      );
  }
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
