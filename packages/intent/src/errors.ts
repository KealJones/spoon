export class LanguageInterpreterError extends Error {
  readonly provider: string;
  readonly cause?: unknown;

  constructor(
    provider: string,
    message: string,
    options?: { cause?: unknown },
  ) {
    super(`${provider}: ${message}`);
    this.name = "LanguageInterpreterError";
    this.provider = provider;
    this.cause = options?.cause;
  }
}

export class OllamaLanguageInterpreterError extends LanguageInterpreterError {
  constructor(message: string, options?: { cause?: unknown }) {
    super("ollama", message, options);
    this.name = "OllamaLanguageInterpreterError";
  }
}

export class CursorLanguageInterpreterError extends LanguageInterpreterError {
  constructor(message: string, options?: { cause?: unknown }) {
    super("cursor", message, options);
    this.name = "CursorLanguageInterpreterError";
  }
}
