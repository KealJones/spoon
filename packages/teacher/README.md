# Spoon Teacher Protocol (`@spoon/teacher`)

This package defines the teacher protocol and provider adapters used when Spoon
cannot resolve a situation locally. Teacher output is structured, provenance-
bound, and provisional until the validation pipeline accepts it and Spoon’s own
evaluation establishes evidence.

## Providers

- `ClaudeTeacher` — invokes Claude’s print-mode CLI.
- `CodexTeacher` — invokes the local Codex CLI.
- `OpenAITeacher` — uses the OpenAI API and `OPENAI_API_KEY`.
- `OllamaTeacher` — uses a local Ollama model.
- `HumanTeacher` — accepts a human-supplied proposal.

The CLI selects a provider with `SPOON_TEACHER` and a model with
`SPOON_TEACHER_MODEL`. Provider failures and malformed proposals are explicit;
the package never marks a teacher response as independently verified by
itself.

The adapter transport is also reusable by other strict structured-output
protocols. In particular, the benchmark **Judge** uses the same Claude/Codex
CLI and OpenAI/Ollama/human backends but supplies its own prompt and response
schema; it is not a Teacher and cannot write to Spoon.

## Library use

```ts
import { OpenAITeacher } from "@spoon/teacher";

const teacher = new OpenAITeacher({ model: "gpt-4.1-mini" });
const proposal = await teacher.propose({
  situation: "what is double 7?",
  context: { concepts: [], procedures: [], assumptions: [] },
  desiredOutput: { type: "object" },
});
console.log(proposal);
```

Prefer the CLI/provider factory for normal Spoon use so request fingerprints,
validation, reliability tracking, and bounded teacher turns stay consistent.

## Development

```bash
pnpm --filter @spoon/teacher test
pnpm --filter @spoon/teacher typecheck
pnpm --filter @spoon/teacher build
```
