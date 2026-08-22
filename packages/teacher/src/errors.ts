export class TeacherError extends Error {
  readonly provider: string;
  readonly cause?: unknown;

  constructor(
    provider: string,
    message: string,
    options?: { cause?: unknown },
  ) {
    super(`${provider}: ${message}`);
    this.name = "TeacherError";
    this.provider = provider;
    this.cause = options?.cause;
  }
}
