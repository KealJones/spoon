import { SourceReliabilityTracker } from "./reliability.js";
import { validateSchema } from "./schema.js";
import { fingerprintTeacherRequest, isJsonValue } from "./shared.js";
import type {
  Clock,
  ProposalValidator,
  TeacherProposal,
  TeacherRequest,
  ValidatedProposal,
  ValidationCheck,
  ValidationStatus,
} from "./types.js";

export interface ProposalValidationPipelineOptions {
  validators?: ProposalValidator[];
  reliabilityTracker?: SourceReliabilityTracker;
  now?: Clock;
}

export class ProposalValidationPipeline {
  readonly #validators: ProposalValidator[];
  readonly #reliabilityTracker?: SourceReliabilityTracker;
  readonly #now: Clock;

  constructor(options: ProposalValidationPipelineOptions = {}) {
    this.#validators = [...(options.validators ?? [])];
    this.#reliabilityTracker = options.reliabilityTracker;
    this.#now = options.now ?? (() => new Date());
  }

  async validate(
    proposal: TeacherProposal,
    request: TeacherRequest,
  ): Promise<ValidatedProposal> {
    const envelopeErrors = validateProposalEnvelope(proposal, request);
    if (envelopeErrors.length > 0) {
      const checks: ValidationCheck[] = [
        {
          validator: "proposal-envelope",
          status: "rejected",
          reason: envelopeErrors.join("; "),
        },
      ];
      if (hasTrustedSource(proposal)) {
        this.#reliabilityTracker?.record(proposal.source, "rejected");
      }
      return {
        content: proposal.content,
        source: proposal.source,
        provenance: proposal.provenance,
        status: "rejected",
        validation: {
          validatedAt: this.#now().toISOString(),
          checks,
        },
      };
    }

    const schemaErrors = validateSchema(
      proposal.content,
      request.desiredOutput,
    );
    const checks: ValidationCheck[] = [
      schemaErrors.length === 0
        ? {
            validator: "proposal-schema",
            status: "verified",
            reason: "Proposal content matches the requested schema",
          }
        : {
            validator: "proposal-schema",
            status: "rejected",
            reason: schemaErrors.join("; "),
          },
    ];

    if (schemaErrors.length === 0) {
      for (const validator of this.#validators) {
        try {
          const result = await validator.validate(proposal, request);
          checks.push({ validator: validator.name, ...result });
        } catch (error) {
          checks.push({
            validator: validator.name,
            status: "provisional",
            reason: `Validator could not complete: ${error instanceof Error ? error.message : String(error)}`,
          });
        }
      }
    }

    const status = this.#status(checks);
    this.#reliabilityTracker?.record(proposal.source, status);
    return {
      content: proposal.content,
      source: proposal.source,
      provenance: proposal.provenance,
      status,
      validation: {
        validatedAt: this.#now().toISOString(),
        checks,
      },
    };
  }

  #status(checks: ValidationCheck[]): ValidationStatus {
    if (checks.some((check) => check.status === "rejected")) return "rejected";
    const independentChecks = checks.slice(1);
    if (
      independentChecks.length > 0 &&
      independentChecks.every((check) => check.status === "verified")
    ) {
      return "verified";
    }
    return "provisional";
  }
}

const PROVIDERS = new Set(["claude", "codex", "openai", "ollama", "human"]);

function validateProposalEnvelope(
  proposal: TeacherProposal,
  request: TeacherRequest,
): string[] {
  const errors: string[] = [];
  if (proposal.status !== "unverified") {
    errors.push("Proposal status must be unverified");
  }
  if (typeof proposal.source !== "string" || proposal.source.trim() === "") {
    errors.push("Proposal source must be a non-empty string");
  }
  const provenance = proposal.provenance;
  if (typeof provenance !== "object" || provenance === null) {
    return [...errors, "Proposal provenance must be an object"];
  }
  if (!PROVIDERS.has(provenance.provider)) {
    errors.push("Proposal provenance provider is unsupported");
  }
  if (provenance.teacher !== proposal.source) {
    errors.push("Proposal provenance teacher must match its source");
  }
  if (
    typeof proposal.source === "string" &&
    !isSourceForProvider(proposal.source, provenance.provider)
  ) {
    errors.push("Proposal source must match its provenance provider");
  }
  if (
    typeof provenance.requestId !== "string" ||
    provenance.requestId.trim() === ""
  ) {
    errors.push("Proposal provenance request id must be non-empty");
  }
  if (
    typeof provenance.generatedAt !== "string" ||
    !isCanonicalTimestamp(provenance.generatedAt)
  ) {
    errors.push("Proposal provenance generated timestamp must be valid");
  }
  if (
    provenance.model !== undefined &&
    (typeof provenance.model !== "string" ||
      provenance.model.trim() === "" ||
      proposal.source !== `${provenance.provider}:${provenance.model}`)
  ) {
    errors.push("Proposal provenance model must match its source");
  }
  if (
    provenance.providerRequestId !== undefined &&
    (typeof provenance.providerRequestId !== "string" ||
      provenance.providerRequestId.trim() === "")
  ) {
    errors.push("Proposal provenance provider request id must be non-empty");
  }
  if (provenance.requestHash !== fingerprintTeacherRequest(request)) {
    errors.push(
      "Proposal provenance request fingerprint must match the request",
    );
  }
  if (provenance.situation !== request.situation) {
    errors.push("Proposal provenance situation must match the request");
  }
  if (provenance.specificQuestion !== request.specificQuestion) {
    errors.push("Proposal provenance question must match the request");
  }
  if (!isJsonValue(proposal.content)) {
    errors.push("Proposal content must be JSON-compatible");
  }
  return errors;
}

function hasTrustedSource(proposal: TeacherProposal): boolean {
  const provenance = proposal.provenance;
  return (
    typeof proposal.source === "string" &&
    proposal.source.trim() !== "" &&
    typeof provenance === "object" &&
    provenance !== null &&
    PROVIDERS.has(provenance.provider) &&
    provenance.teacher === proposal.source &&
    isSourceForProvider(proposal.source, provenance.provider)
  );
}

function isSourceForProvider(source: string, provider: string): boolean {
  const prefix = `${provider}:`;
  return source.startsWith(prefix) && source.slice(prefix.length).trim() !== "";
}

function isCanonicalTimestamp(value: string): boolean {
  const timestamp = Date.parse(value);
  return (
    Number.isFinite(timestamp) && new Date(timestamp).toISOString() === value
  );
}
