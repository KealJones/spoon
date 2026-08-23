import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { parseBenchmarkFixture } from "../src/benchmark.js";

async function fixture(name: string): Promise<unknown> {
  return JSON.parse(
    await readFile(`../../benchmarks/fixtures/${name}.json`, "utf8"),
  ) as unknown;
}

test("developmental fixtures preserve acquisition, retention, and held-out phases", async () => {
  const parsed = parseBenchmarkFixture(await fixture("AMBIG-001"));
  assert.equal(parsed.probes.length, 1);
  const probe = parsed.probes[0]!;
  assert.equal(probe.acquisition.teacherMode, "on");
  assert.equal(probe.retention.teacherMode, "off");
  assert.equal(probe.variants[0]?.teacherMode, "off");
  assert.equal(probe.acquisition.expectedOutcome?.type, "clarify");
});

test("procedure selection fixtures support a second Teacher-ON teaching turn", async () => {
  const parsed = parseBenchmarkFixture(await fixture("INTERF-001"));
  const probe = parsed.probes[0]!;
  assert.equal(probe.additionalAcquisition.length, 1);
  assert.equal(probe.additionalAcquisition[0]?.id, "teach-zorp");
  assert.equal(probe.additionalAcquisition[0]?.teacherMode, "on");
  assert.equal(probe.retention.teacherMode, "off");
  assert.equal(probe.variants.length, 2);
  assert.ok(probe.variants.every((variant) => variant.teacherMode === "off"));
});

test("the runner rejects the retired seed fixture shape", () => {
  assert.throws(
    () =>
      parseBenchmarkFixture({
        version: 1,
        name: "retired",
        probes: [],
      }),
    /schemaVersion 1 experiment format/,
  );
});
