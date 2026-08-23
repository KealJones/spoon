import type {
  ProposalValidationPipeline,
  ProposalValidationPipelineOptions,
} from "./validation.js";

export type JsonPrimitive = null | boolean | number | string;

export type JsonValue =
  JsonPrimitive | JsonValue[] | { [key: string]: JsonValue };

export interface JsonObject {
  [key: string]: JsonValue;
}

export type JsonSchemaType =
  "null" | "boolean" | "object" | "array" | "number" | "integer" | "string";

export interface ProposalSchema {
  $id?: string;
  $schema?: string;
  title?: string;
  description?: string;
  type?: JsonSchemaType | JsonSchemaType[];
  enum?: JsonValue[];
  const?: JsonValue;
  properties?: Record<string, ProposalSchema | boolean>;
  required?: string[];
  additionalProperties?: boolean | ProposalSchema;
  items?: ProposalSchema | boolean;
  minItems?: number;
  maxItems?: number;
  uniqueItems?: boolean;
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  minimum?: number;
  maximum?: number;
  exclusiveMinimum?: number;
  exclusiveMaximum?: number;
  allOf?: Array<ProposalSchema | boolean>;
  anyOf?: Array<ProposalSchema | boolean>;
  oneOf?: Array<ProposalSchema | boolean>;
  not?: ProposalSchema | boolean;
}

export interface KnowledgeContext {
  concepts?: JsonObject[];
  relationships?: JsonObject[];
  procedures?: JsonObject[];
  episodes?: JsonObject[];
  [key: string]: JsonValue | undefined;
}

export interface TeacherRequest {
  situation: string;
  context: KnowledgeContext;
  specificQuestion?: string;
  desiredOutput: ProposalSchema;
}

/**
 * Provider adapters are shared by distinct structured-output protocols. The
 * default builder is the Spoon Teacher prompt; callers such as the benchmark
 * Judge provide their own protocol-specific builder and system prompt.
 */
export type PromptBuilder = (request: TeacherRequest) => string;

export interface ProviderPromptOptions {
  systemPrompt?: string;
  promptBuilder?: PromptBuilder;
}

export type ProviderKind =
  "claude" | "codex" | "cli" | "openai" | "ollama" | "human";

export interface ProposalProvenance {
  provider: ProviderKind;
  teacher: string;
  model?: string;
  requestId: string;
  requestHash?: string;
  providerRequestId?: string;
  generatedAt: string;
  situation: string;
  specificQuestion?: string;
}

export interface TeacherProposal {
  content: JsonValue;
  source: string;
  status: "unverified";
  provenance: ProposalProvenance;
}

export type ValidationStatus = "verified" | "rejected" | "provisional";

export interface ValidationCheck {
  validator: string;
  status: ValidationStatus;
  reason: string;
  evidence?: JsonValue;
}

export interface ProposalValidation {
  validatedAt: string;
  checks: ValidationCheck[];
}

export interface ValidatedProposal extends Omit<TeacherProposal, "status"> {
  status: ValidationStatus;
  validation: ProposalValidation;
}

export interface ProposalValidatorResult {
  status: ValidationStatus;
  reason: string;
  evidence?: JsonValue;
}

export interface ProposalValidator {
  name: string;
  validate(
    proposal: TeacherProposal,
    request: TeacherRequest,
  ): ProposalValidatorResult | Promise<ProposalValidatorResult>;
}

export interface SourceReliability {
  source: string;
  total: number;
  verified: number;
  rejected: number;
  provisional: number;
  score: number;
}

export interface Teacher {
  propose(request: TeacherRequest): Promise<TeacherProposal>;
  reliability(): SourceReliability;
  validationPipeline(
    options?: Omit<ProposalValidationPipelineOptions, "reliabilityTracker">,
  ): ProposalValidationPipeline;
}

export interface CommandInvocation {
  command: string;
  args: string[];
  cwd?: string;
}

export interface CommandResult {
  exitCode: number;
  stdout: string;
  stderr: string;
}

export type CommandRunner = (
  invocation: CommandInvocation,
) => Promise<CommandResult>;

export type Fetch = (
  input: string | URL,
  init?: RequestInit,
) => Promise<Response>;

export type HumanPrompt = (message: string) => Promise<string | JsonValue>;

export type Clock = () => Date;
export type IdFactory = () => string;
