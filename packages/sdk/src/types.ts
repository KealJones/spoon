export type JsonValue =
  null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

export interface ConceptInput {
  name: string;
  description?: string;
  [key: string]: JsonValue | undefined;
}

export interface EpisodeFilter {
  since?: number;
  until?: number;
  limit?: number;
  outcome?: "success" | "failure" | "any";
  rung?: string;
  conceptId?: string;
}

export interface RpcTransport {
  request<T>(method: string, params: unknown): Promise<T>;
  close?(): void;
}

export interface CycleAssumption {
  description: string;
  basis: string;
  concept?: string | null;
}

export interface CycleBudget {
  maxExecSteps: number;
  maxContextItems: number;
  maxTeacherTurns: number;
}

export interface CycleInput {
  situation: string;
  environment: Record<string, JsonValue>;
  assumptions: CycleAssumption[];
  budget: CycleBudget;
  teacherAllowed: boolean;
}

export interface TeacherRequestWire {
  situation: string;
  context: Record<string, JsonValue>;
  specificQuestion?: string;
  desiredOutput: Record<string, JsonValue>;
}

export interface ProposalProvenanceWire {
  provider: "claude" | "openai" | "ollama" | "human";
  teacher: string;
  model?: string;
  requestId: string;
  requestHash?: string;
  providerRequestId?: string;
  generatedAt: string;
  situation: string;
  specificQuestion?: string;
}

export interface TeacherProposalWire {
  content: JsonValue;
  source: string;
  status: "unverified";
  provenance: ProposalProvenanceWire;
  validation?: {
    status: "verified" | "rejected" | "provisional";
    validatedAt: string;
    checks: Array<{
      validator: string;
      status: "verified" | "rejected" | "provisional";
      reason: string;
      evidence?: JsonValue;
    }>;
  };
}

export interface NeedTeacherProgress {
  status: "need_teacher";
  cycleId: string;
  request: TeacherRequestWire;
}

export interface CompletedCycleProgress {
  status: "completed";
  cycleId: string;
  disposition: "verified" | "provisional" | "abstained";
  answer: JsonValue;
  episode: Record<string, JsonValue>;
}

export type CycleProgress = NeedTeacherProgress | CompletedCycleProgress;
