# Lessons (corrections from Keal, newest first)

## 2026-09-02 Subagent model cost
- Every subagent spawned with the default `inherit` ran on the session model
  (Fable), the most expensive one, across 17 rounds. Keal caught it.
- Rule: always pass `model` explicitly. `claude-4.6-sonnet-medium-thinking` for
  mechanical rounds (rules, wiring, tests, HTML, docs, benches). Opus-class only
  for a design-heavy round, and say so before spawning.
- Rule: never run more than two subagents at once; each one compiles the whole
  workspace and runs Ollama.
- Rule: skip live-Ollama benchmarks in a round unless the change is in the ears
  or mouth. Offline tests are enough to verify wiring.
