import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import type { Readable, Writable } from "node:stream";

import type { RpcTransport } from "./types.js";

interface PendingRequest {
  resolve(value: unknown): void;
  reject(error: unknown): void;
}

interface RpcResponse {
  jsonrpc: "2.0";
  id: number;
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
}

export class JsonRpcError extends Error {
  constructor(
    public readonly code: number,
    message: string,
    public readonly data?: unknown,
  ) {
    super(message);
    this.name = "JsonRpcError";
  }
}

export class StreamTransport implements RpcTransport {
  private nextId = 1;
  private buffer = "";
  private readonly pending = new Map<number, PendingRequest>();
  private closed = false;

  constructor(
    private readonly input: Readable,
    private readonly output: Writable,
  ) {
    input.setEncoding("utf8");
    input.on("data", (chunk: string) => this.consume(chunk));
    input.on("end", () => this.close());
    input.on("error", (error) => this.failAll(error));
    output.on("error", (error) => this.failAll(error));
  }

  request<T>(method: string, params: unknown): Promise<T> {
    if (this.closed) {
      return Promise.reject(new Error("JSON-RPC transport is closed"));
    }

    const id = this.nextId++;
    const payload = JSON.stringify({ jsonrpc: "2.0", id, method, params });

    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, {
        resolve: (value) => resolve(value as T),
        reject,
      });
      this.output.write(`${payload}\n`);
    });
  }

  close(): void {
    if (this.closed) return;
    this.closed = true;
    this.failAll(new Error("JSON-RPC transport closed"));
  }

  private consume(chunk: string): void {
    this.buffer += chunk;
    while (true) {
      const newline = this.buffer.indexOf("\n");
      if (newline < 0) return;

      const line = this.buffer.slice(0, newline).trim();
      this.buffer = this.buffer.slice(newline + 1);
      if (line.length === 0) continue;

      let response: RpcResponse;
      try {
        response = JSON.parse(line) as RpcResponse;
      } catch (error) {
        this.failAll(new Error(`Invalid JSON-RPC response: ${String(error)}`));
        continue;
      }

      const pending = this.pending.get(response.id);
      if (!pending) continue;
      this.pending.delete(response.id);

      if (response.error) {
        pending.reject(
          new JsonRpcError(
            response.error.code,
            response.error.message,
            response.error.data,
          ),
        );
      } else {
        pending.resolve(response.result);
      }
    }
  }

  private failAll(error: unknown): void {
    for (const request of this.pending.values()) request.reject(error);
    this.pending.clear();
  }
}

export class StdioTransport extends StreamTransport {
  private constructor(private readonly child: ChildProcessWithoutNullStreams) {
    super(child.stdout, child.stdin);
  }

  static spawn(
    command = "target/debug/spoon-server",
    args: string[] = [],
  ): StdioTransport {
    const child = spawn(command, args, { stdio: ["pipe", "pipe", "pipe"] });
    return new StdioTransport(child);
  }

  override close(): void {
    super.close();
    this.child.kill();
  }
}
