import assert from "node:assert/strict";
import test from "node:test";
import { PassThrough } from "node:stream";

import { JsonRpcError, StreamTransport } from "../src/index.js";

test("stream transport correlates newline-delimited responses", async () => {
  const input = new PassThrough();
  const output = new PassThrough();
  const transport = new StreamTransport(input, output);

  const pending = transport.request<{ value: number }>("ping", { value: 41 });
  const request = JSON.parse(output.read().toString()) as { id: number };
  input.write(
    `${JSON.stringify({ jsonrpc: "2.0", id: request.id, result: { value: 42 } })}\n`,
  );

  assert.deepEqual(await pending, { value: 42 });
  transport.close();
});

test("stream transport rejects structured JSON-RPC errors", async () => {
  const input = new PassThrough();
  const output = new PassThrough();
  const transport = new StreamTransport(input, output);

  const pending = transport.request("missing", {});
  const request = JSON.parse(output.read().toString()) as { id: number };
  input.write(
    `${JSON.stringify({ jsonrpc: "2.0", id: request.id, error: { code: -32601, message: "method not found" } })}\n`,
  );

  await assert.rejects(pending, (error: unknown) => {
    return error instanceof JsonRpcError && error.code === -32601;
  });
  transport.close();
});
