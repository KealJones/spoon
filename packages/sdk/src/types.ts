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
