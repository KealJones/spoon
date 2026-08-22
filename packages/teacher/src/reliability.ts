import type { SourceReliability, ValidationStatus } from "./types.js";

interface MutableReliability {
  verified: number;
  rejected: number;
  provisional: number;
}

export class SourceReliabilityTracker {
  readonly #sources = new Map<string, MutableReliability>();

  record(source: string, status: ValidationStatus): SourceReliability {
    const counts = this.#sources.get(source) ?? {
      verified: 0,
      rejected: 0,
      provisional: 0,
    };
    counts[status] += 1;
    this.#sources.set(source, counts);
    return this.get(source);
  }

  get(source: string): SourceReliability {
    const counts = this.#sources.get(source) ?? {
      verified: 0,
      rejected: 0,
      provisional: 0,
    };
    const total = counts.verified + counts.rejected + counts.provisional;

    // A Beta(1, 1) prior avoids treating a new source as perfectly reliable.
    const score =
      (1 + counts.verified + counts.provisional * 0.5) / (2 + total);
    return { source, total, ...counts, score };
  }

  all(): SourceReliability[] {
    return [...this.#sources.keys()].sort().map((source) => this.get(source));
  }
}
