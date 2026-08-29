# Spoon Primitive and Capability Inventory

This is the living completion checklist for Spoon's executable substrate. It
tracks what Spoon can actually execute, not names that merely appear in a type
or plan.

Snapshot: 2026-08-28

An operation is only real when it is declared, evaluated, nameable by a lesson,
advertised accurately, and tested. Those five facts used to live in five
hand-maintained places, which is how 96 operations came to be evaluable but
unusable. They are now derived from one table and checked by the guards in
`crates/spoon-exec/tests/intrinsic_coverage.rs`, so a row below that claims an
operation works is backed by a test that fails if it stops working.

## Legend and boundary

- `[x]` implemented: executable through a public path with meaningful tests.
- `[~]` partial: useful machinery exists, but the operation is incomplete,
  inaccessible to learned procedures, fixture-only, or missing important safety
  and contract semantics.
- `[ ]` missing: no usable implementation yet.
- **Native pure**: deterministic, authority-free runtime semantics.
- **Native effect**: the smallest permission-enforcing bridge to host behavior.
- **Seeded/acquired**: ordinary procedures and knowledge composed from native
  mechanisms. It is inspectable, revisable, exportable, and carries no authority.

### Evidence ladder

For every status claim, identify the highest proven level: **D** declared, **C**
compiled, **U** unit-executed, **R** publicly reachable, **I** integrated into
the real workflow, **A** adversarially tested, **P** production-real. A mock,
fixture, test-only adapter, simulated receipt, or unused public method must be
labeled as such. `[x]` requires at least `R`; effectful production-readiness
requires `P`. The implementation plan's Reality Gate defines the full policy.

The set of possible host interfaces is unbounded. The goal is therefore not a
native primitive for every application. The native effect families must be
complete enough that new interfaces can be added as typed, locally authorized
capability adapters without changing the evaluator.

## Reality audit log

| Date | Claim audited | Highest contiguous proof | Result |
| --- | --- | --- | --- |
| 2026-08-23 | Pure intrinsic text/JSON/path slice | Public/integrated/adversarial local proof | Bounded tokenizer, JSON Pointer strict/optional reads and immutable set/delete, null-only coalesce, structural find-index, and `map_from_entries` execute through core/evaluator/Teacher rich lessons; focused evaluator/engine tests and a real Rust-stdio SDK Teacher-OFF retention test pass. Broader language semantics and the remaining inventory are still incomplete. |
| 2026-08-23 | File read/write are usable through the app | Unit-executed | Real scoped helper logic and adversarial tests exist; no server/SDK/learned-procedure invocation path. Downgraded to partial. |
| 2026-08-23 | Sandboxed execution is usable through the app | Unit-executed fixture | Policy and deterministic fixture run; implementation explicitly spawns no process and registers no real sandbox adapter. Partial only. |
| 2026-08-23 | Capability grants and invocation are integrated | Unit/direct API | Durable grants and injected-adapter engine tests exist; no public invocation transport or cognitive-cycle selection. Downgraded to partial. |
| 2026-08-23 | Scoped file read/write through `capability.invoke` | Failure/adversarial tested locally | JSON-RPC reaches a real temporary-directory adapter with persistent grants, next-call revocation, malformed/bounds denial, redacted public receipts, symlink-escape rejection, and honest unsupported-primitive failure. Production deployment and cognitive-cycle selection are not evidenced. |
| 2026-08-23 | Scoped file bridge after concurrent intrinsic work | Public/integrated/adversarial local proof | `cargo test -p spoon-server --test rpc capability_invoke` and the full Rust workspace gate pass after the intrinsic work settled. Real SDK invocation, cognitive-cycle selection, and production deployment remain missing. |
| 2026-08-28 | This document's own "missing" rows for logarithms, trigonometry, bitwise, regex, edit distance, templates, set operations, take/drop/chunk/window, base64/hex/URL coding, hashing, predicates, conversions, and date arithmetic | Unit-executed and lesson-nameable | The rows were stale about the evaluator and accidentally right about the system. All 167 operations were evaluated, but 96 of them could not be named by a Teacher lesson. One table now generates the enum, the lesson name, the reverse lookup, and `ALL`. Tests covering the previously untested 98 operations then found real defects: math ops leaking NaN/inf, `gcd`/`numeric_to_fixed` panicking, `to_int` saturating silently, `date_from_parts` accepting February 30, URL decode treating bytes as Latin-1, `text_reverse` splitting graphemes, `text_format` rescanning substitutions, and `set_union` keeping left-side duplicates. Those are fixed. The RNG and clock still cannot be seeded or injected, so procedures that call them are not reproducible. |

This log records material claim corrections. Focused evidence matrices may live
in task scratchpads or benchmark reports; the inventory always reflects their
weakest supported conclusion.

## 1. Portable execution and safety

- [x] Neutral values: null, boolean, integer, float, text, list, string-keyed map.
- [ ] Explicit byte/binary value.
- [ ] Arbitrary-precision integer and exact decimal value.
- [ ] Tagged result/option/error values for recoverable procedure logic.
- [x] Literals, variables, lexical `let`, blocks, and conditionals.
- [x] Arithmetic: add, subtract, multiply, divide, modulo.
- [~] Arithmetic safety: integer overflow and division-by-zero are typed; the
  new numeric intrinsics reject non-finite inputs/results, while legacy mixed
  float arithmetic still needs a complete non-finite policy.
- [x] Numeric standard operations: bounded `numeric_abs`, `numeric_sign`,
  `numeric_min`/`numeric_max`, `numeric_clamp`, floor/ceil/round/truncate,
  checked integer power, finite float power, strict integer
  quotient/remainder, logarithms, roots, trigonometry, gcd/lcm, and hypot.
  Non-finite inputs and results are `InvalidNumber`. Exact decimal/rational
  arithmetic is still missing.
- [ ] Exact decimal/rational arithmetic and configurable rounding modes.
- [x] Bitwise integer operations and shifts. Shift distance outside 0..=63 is
  an error. Left shift wraps the bit pattern rather than reporting overflow.
- [x] Equality, inequality, numeric/text ordering, boolean and/or/not.
- [ ] Explicit total/deep ordering across neutral values.
- [x] Exact procedure calls and version-pinned replay; `CallExact` additionally
  pins learned pure-procedure dependencies to the admitted revision.
- [x] Per-evaluation step budget.
- [ ] Independent call-depth, expression-depth, allocation/item, and output-byte budgets.
- [~] A timeout error variant is declared, but no evaluator or effect path enforces
  wall-clock deadlines or cooperative cancellation.
- [x] Procedure-call traces with inputs, outputs, versions, failures, and contract checks.
- [~] Contracts: executable boolean conditions work; typed schemas, effects, richer
  invariants, and failure postconditions are incomplete.
- [x] Procedure tests are represented in core procedure data.
- [~] Multiple learned test cases and generated boundary/counterexample execution are
  not yet part of lesson admission.
- [~] A versioned bounded `pure_expr_v2` Teacher/admission grammar supports pure
  control flow, collection forms, versioned intrinsics, and request-local
  exact-version pure dependency aliases. Engine tests, one real Codex CLI
  teach/Teacher-OFF reuse smoke, and a real Rust-stdio SDK quote-bound rich
  letter-count retention test pass. The quote binder is ordered explicit syntax,
  not a general semantic parser; broad provider/public-SDK proof remains missing.
- [x] Legacy `pure_rpn_v1` scalar lesson compiler.
- [ ] Structured recoverable exceptions, try/result matching, assertions, and explicit
  failure construction in learned procedures.
- [ ] Deterministic parallel map/task primitives with resource accounting.

## 2. Text and Unicode — native pure

- [~] Text literals, equality, ordering, truthiness, and concatenation exist.
- [x] Byte length, Unicode scalar length, and grapheme-cluster length are separate,
  versioned evaluator intrinsics with composed-Unicode tests.
- [~] Grapheme-aware split-on-empty and list join execute; explicit scalar iteration
  and non-empty split limits remain missing.
- [x] Trim, trim-start/end, bounded repetition, and concatenate-many execute.
- [x] Unicode normalization (NFC/NFD/NFKC/NFKD) executes with explicit form names.
- [~] Locale-independent Unicode lower/upper casing executes; full Unicode case folding is missing.
- [~] Contains, prefix/suffix, replace, first-index, non-overlapping count, and
  grapheme substring execute; reverse-index and richer span semantics remain missing.
- [~] Bounded source spans and tokenizer retain UTF-8 byte offsets in the core
  language substrate; scalar/grapheme offsets and normalization provenance are missing.
- [x] Deterministic word/number/whitespace/punctuation/symbol tokenizer is a
  bounded `text_tokenize` procedure intrinsic. It returns exact UTF-8 source
  text plus `startByte`/`endByte` spans in neutral token maps; a real Rust-stdio
  SDK lesson counts held-out word tokens with Teacher OFF. It is lexical only,
  not intent inference or parsing.
- [x] Bounded regular expressions with match, capture, and replace-all. An
  invalid pattern is an error. The regex crate is backtracking, not a
  custom non-backtracking engine, so a pathological pattern is a resource
  cost rather than a guaranteed linear scan.
- [ ] Glob/pattern matching with explicit syntax and limits.
- [x] Bounded Levenshtein edit distance over Unicode scalars. Phonetic
  transforms are still missing.
- [x] Format/template operation that substitutes named placeholders from a map
  and does not rescan substituted values. There is no escape for a literal
  `{name}` other than omitting that key.
- [ ] Parsing and rendering of escaped strings.

The versioned intrinsic vocabulary is available to `pure_expr_v2` lessons in
Engine tests. Broader public rich-lesson transport/provider evidence remains
tracked in section 1.

## 3. Collections and objects — native pure

- [x] List construction and immutable list values.
- [~] Map values and literal maps exist; bounded computed object construction via
  `map_from_entries` now executes, while richer map comprehensions remain missing.
- [x] List index and map field access.
- [x] List map, filter, and reduce with lexical iteration scopes and step charging.
- [x] Generic grapheme/list/map length.
- [x] Deterministic map keys, values, entries, copy set/delete, and right-biased merge execute.
- [x] Strict get versus optional get distinguishes missing from present null;
  optional access converts absence only and preserves malformed/type errors.
- [~] Immutable copy set, delete, and shallow right-biased merge execute; deep merge is missing.
- [x] List contains, equality-count, structural find-index, any, all, and
  partition. Predicate find over an expression (rather than a field name)
  is still missing.
- [x] Slice, reverse, take, drop, chunk, window, and bounded end-exclusive
  range. Chunk and window size 0 are errors or empty rather than panics.
- [x] Stable deterministic total-order sort executes. Mixed int/float columns
  sort by type rank, not numerically.
- [x] Stable unique/deduplicate, and set union/intersection/difference/
  subset. Union deduplicates both operands. Intersect and difference keep
  left-operand duplicates, matching their filter-shaped descriptions.
- [x] One-level flatten, zip, enumerate, and group-by. Unzip and transpose
  remain missing. Group-by stringifies keys, so `2` and `"2"` collide.
- [~] Min/max by field, with ties going first for min and last for max.
  Sum/product/average and frequency tables are still missing.
- [ ] Bounded cartesian product and combinatorics.
- [ ] Deterministic map/filter/reduce over maps as key/value entries.

## 4. JSON, paths, schemas, and data formats — native pure

- [~] JSON is procedure-executable through the rich Teacher lesson grammar for
  parse/stringify and bounded path operations; schema validation, patch/update,
  and broader format support remain missing.
- [x] Bounded JSON parse into neutral values with signed-integer and depth/byte checks.
- [x] Deterministic JSON stringify from neutral values with non-finite/depth/output checks.
- [x] Dot/bracket property paths: `user.profile.name`, `items[0].id`, and quoted keys.
- [x] Strict and optional path access with typed malformed/type/missing outcomes;
  optional access converts absence only.
- [x] Bounded JSON Pointer reads, including root access, arrays, `~0`/`~1`
  escaping, strict/optional missing behavior, and malformed/type denial, are
  procedure-executable and covered through a real Rust-stdio SDK lesson.
- [~] Immutable JSON Pointer set/delete updates execute with root replacement,
  escaped keys, array replacement/removal, explicit missing/type errors, and
  input preservation; broader dot/bracket updates and patch formats remain.
- [ ] JSON Patch and Merge Patch as bounded pure transforms.
- [~] Capability schemas support a JSON-schema subset at admission/invocation.
- [ ] Procedure-accessible schema validation with structured violations.
- [x] Type name, predicates (`is_null` through `is_numeric`), bounded
  parse-int/parse-float/parse-bool/to-text, `to_int`/`to_float`/`to_bool`,
  and variadic null-only `coalesce`. Predicates are representation checks,
  not value checks: `is_int(2.0)` is false. `to_int` refuses non-finite and
  out-of-range floats rather than saturating.
- [x] SHA-256 and MD5 hex digests, plus hex and binary integer rendering that
  round-trips through a signed magnitude encoding. Canonical deterministic
  encoding of arbitrary values is still missing.
- [x] Base64, hex, and URL encode/decode of UTF-8 text. Base32 and a distinct
  binary value type are still missing. URL decode rejects malformed percent
  escapes and reassembles UTF-8 rather than Latin-1.
- [ ] CSV/TSV parse/stringify with dialect metadata.
- [ ] Query-string and form-data transforms.
- [ ] YAML/TOML/XML/HTML parsing through sandboxed/acquired adapters rather than the
  trusted evaluator unless a minimal safe format later proves foundational.

## 5. Identifiers, time, units, and algorithms — pure or observed

- [x] Random UUID v4 generation, plus integer/float/choice/shuffle/sample.
  None of these can be seeded, so a learned procedure that calls them cannot
  be replayed or regression-tested. That is a real architectural gap, not
  a missing operation.
- [ ] URL parse/resolve/normalize and origin/host/path extraction.
- [ ] IP/CIDR parsing and containment without performing network access.
- [ ] MIME/media-type parsing.
- [~] Dates are unix-epoch integers, not a distinct value type. `date_from_parts`
  rejects days the month does not have, including the Gregorian leap-year
  cases. Arithmetic is seconds/minutes/hours/days only, no months or timezones.
- [x] Pure date formatting, part extraction, and duration arithmetic over the
  epoch representation. Timezone-aware values are still missing.
- [x] `date_now` is an evaluator intrinsic over `SystemTime::now`. The clock
  cannot be injected, with the same reproducibility cost as the unseedable
  RNG. The native `clock` observation still exists separately.
- [ ] Timezone database capability with version provenance.
- [ ] Unit/quantity values, dimensional checks, and exact conversions.
- [x] SHA-256 and MD5 as pure text-to-hex operations. HMAC signing lives in
  `spoon-secret` as a local identity, not as a procedure intrinsic. Secret
  material never becomes a Spoon value.
- [ ] General graph traversal, shortest path, topological sort, and cycle detection
  over neutral graph-shaped values.
- [ ] Bounded search/optimization primitives and deterministic priority queues.

## 6. Network mechanism — native effect

- [x] Exact-host permission, scheme/port/method allowlists, byte bounds, and
  redacted receipts. A caller never supplies a URL: the target is assembled
  from host policy plus a validated path and query. Not yet reached from a
  public RPC path.
- [x] Scheme, port, method, and redirect scopes. `https` only by default;
  `http` solely for an explicitly configured host. Redirects are off by
  default, followed by the adapter (not reqwest), revalidated every hop, and
  only GET/HEAD are followed.
- [x] Request headers with mandatory secret redaction (`authorization`,
  `cookie`, `proxy-authorization`, `x-api-key`). Secret-bearing headers never
  appear in the receipt, the output, or error text.
- [x] Query parameters, body, response status/headers/body, and a streaming
  byte cap that aborts mid-download rather than buffering then rejecting.
- [x] Connect and total timeouts derived from `bounds.max_millis`.
- [x] DNS rebinding and private-network protections. The adapter resolves,
  refuses unless every address is public (or explicitly permitted), and
  hands only that set to the connector. Proven against a `.invalid` host no
  resolver can answer. An HTTP proxy in the process environment is not
  covered. The check and connect are still two steps.
- [~] TLS is rustls via reqwest. Peer/certificate provenance is not surfaced
  in the output because the blocking client does not expose the chain.
- [~] Streaming download with a total budget is enforced. Streaming upload
  with per-chunk budgets is not.
- [ ] Pagination, retries, backoff, idempotency, and rate-limit handling as seeded
  procedures over the request primitive.
- [ ] WebSocket/server-sent-event/stream subscription adapter family.
- [x] Offline in-memory transport sharing the exact production contract.

## 7. Filesystem mechanism — native effect

- [x] `capability.invoke` reaches a real regular-file read through a server-configured
  logical binding; durable grants, host scope/bounds, symlink escape, response bounds,
  and redacted public receipts are checked (A, local temporary-directory integration).
- [x] `capability.invoke` reaches a real regular-file write through the same boundary;
  missing/revoked grants, request bounds, symlink targets, and unsupported adapters fail
  without an ambient fallback (A, local temporary-directory integration).
- [ ] Atomic write/replace with fsync policy and failure receipt.
- [ ] Scoped directory listing with item/depth/name-byte bounds.
- [ ] Metadata/stat, existence, canonical identity, and file fingerprint observation.
- [ ] Create directory, copy, move/rename, and explicitly recoverable delete/trash.
- [ ] Fine-grained append versus overwrite permissions.
- [ ] Byte-range and streaming reads/writes.
- [ ] Safe patch primitive with expected-content hash and atomic conflict failure.
- [ ] Temporary file/directory allocation with lifecycle cleanup receipts.
- [ ] File watching/change subscriptions with event coalescing and overflow signals.
- [ ] Archive list/extract/create with zip-slip, link, expansion-ratio, item, and byte limits.
- [ ] Filesystem transaction/worktree snapshot adapter for reversible coding changes.

## 8. Observation mechanism — native effect

- [~] Named observation targets, target-specific permission, bounds, and receipts exist.
- [~] `clock` is the only concrete native observation target.
- [ ] Cryptographic randomness/entropy observation with byte bounds.
- [ ] Monotonic time and elapsed-duration observation.
- [ ] Operating-system/platform/architecture observation.
- [ ] Process working-directory observation, separated from file authority.
- [ ] Whitelisted environment-key presence/value observation with secret classification.
- [ ] Terminal dimensions/capabilities and locale observation.
- [ ] Resource usage: CPU, memory, disk, and network counters.
- [ ] Process/sandbox status and exit observation.
- [ ] User-confirmation/input request with explicit interaction receipt.
- [ ] Clipboard, screen, camera, microphone, location, and device sensors as separate,
  highly scoped adapters—not one generic ambient sensor permission.
- [ ] External state snapshots with freshness, source identity, timestamp, and confidence.

## 9. Sandboxed execution mechanism — native effect

- [x] A real operating-system sandbox runner in `spoon-sandbox`. Digest-pinned
  executable, empty environment plus an allowlist, working directory confined
  to a configured root, byte-bounded output, and a wall-clock bound that kills
  the process group. macOS confinement uses `sandbox-exec` and denies network.
- [x] Exact executable identity via content digest, and an allowed argument schema.
- [x] Stdin, stdout, stderr, and exit status captured in the adapter receipt.
- [~] Wall-time and output-byte limits are enforced. CPU, memory, process-count,
  open-file, and disk quotas are not.
- [ ] Explicit filesystem mount/read/write policy beyond the working-directory root.
- [x] Network/offline policy independent from the network primitive: the default
  profile denies network. Proven by a test that the child cannot connect.
- [x] Minimal allowlisted environment. Secret-reference injection is not wired.
- [x] Working-directory confinement. Input/output artifact declarations are not.
- [x] Timeout kills the child and its process group rather than just the leader.
- [ ] Container/VM/WASI adapter support behind the same invocation contract.
- [~] Tests use real children, not a fixture executor. Platform compatibility
  fingerprints are not recorded.
- [ ] Determinism/replay classification based on declared inputs and captured artifacts.

## 10. Secrets, identity, and trust — native effect/policy

- [x] Opaque `SecretRef` in `spoon-secret`. It cannot carry a value, and
  `Debug`/`Display`/serde cannot leak one. It is not yet a Spoon `Value`
  variant, so a procedure cannot name a secret.
- [x] Metadata lookup (namespace, name, version, grant status) without
  disclosing the value.
- [x] Just-in-time resolution through a `SecretResolver`, in-memory or
  environment-allowlist. Out-of-scope use never touches material. Not yet
  called from a host adapter in the invoke path.
- [x] A `Redactor` replaces resolved values in strings, nested JSON, and URL
  query parameters. It is not yet installed on prompts, episodes, or receipts.
- [x] Grants carry primitive/target/purpose scope, mandatory expiry, rotation
  that supersedes a version, and revocation. Stale versions fail.
- [x] HMAC-SHA-256 local signing identity. Key material never becomes a Spoon
  value. Not a publisher identity: no non-repudiation, no public verifiability.
- [ ] Local user/agent/service identity assertions and authentication receipts.
- [x] Imported capability bundles do not transfer grants automatically.
- [x] Mandatory local denials override permission modes.
- [~] Provenance identities and references exist; cryptographic publisher identity and
  transparent signature verification are incomplete.

## 11. Capability acquisition, packaging, and lifecycle

- [x] Typed capability specifications with procedures, schemas, effects, permissions,
  resource bounds, tests, dependencies, compatibility, and provenance.
- [x] Deterministic content-addressed reconstructible bundles.
- [x] Export/import round trips and bundle size/count/dependency validation.
- [x] Imported capabilities enter Provisional/quarantine and receive no grants.
- [x] Local reconstruction and revalidation APIs.
- [~] Local durable grants, revocation, and invocation-time permission checks reach a
  public JSON-RPC/SDK invocation path and real configured scoped-file adapter; automatic
  learned-procedure/cognitive-cycle selection is still missing.
- [x] Dependency closure and exact content/procedure resolution.
- [~] Discovery can build typed candidates from supplied specifications; autonomous
  interface inspection and candidate synthesis are not wired into the cognitive cycle.
- [ ] Multiple candidate generation and comparison.
- [ ] Generated boundary, property, fuzz, mutation, and adversarial test production.
- [ ] Candidate workspace with atomic admission/rejection and retained failure evidence.
- [ ] Capability repair, supersession, migration, rollback, and compatibility negotiation.
- [ ] Dependency resolver with offline cache, lockfile, conflict explanation, and cycle UI.
- [x] Seed forge: load a curriculum, teach a clean in-memory instance, run
  Teacher-OFF gates that abort if the engine asks, inspect expected
  structures, export under an `exportPrivacy` filter, reconstruct in a
  second clean instance, and re-run the gates. Publication signing is a
  documented seam (`ReportSigner`), not implemented here: HMAC in
  `spoon-secret` is a local identity, not a publisher signature.
- [ ] Signed seed/capability registry protocol and transparent artifact mirroring.

## 12. Language and meaning — primarily seeded/acquired

- [~] Bounded native UTF-8 token stream with deterministic word/number/whitespace/
  punctuation/symbol tokens and byte-accurate source spans. No grapheme tokens or
  normalization transform yet.
- [~] Serializable intent-frame, slot, scope, and ambiguity values. No learned mapping,
  entity/reference resolution, or clarification policy yet.
- [~] Serializable dialogue-act/move values. No durable conversational-state model yet.
- [ ] Core compositional grammar and semantic construction procedures.
- [ ] Phrase/paraphrase-to-intent learning with confidence and competing hypotheses.
- [ ] Reference resolution grounded in conversation and observed entities.
- [ ] Correction, negation, modality, temporal scope, and pragmatic implication handling.
- [~] Response plan with grounded/unsupported claims, evidence/provenance references,
  uncertainty, tone, and content-free formatting variation. Requested actions and disclosure
  policy are not represented yet.
- [~] Deterministic no-model response-plan renderer: a bounded public `language.render`
  Server/SDK workflow emits only supplied evidence-referenced claim text, omits unsupported
  claims, rejects evidence-free claims, and changes only plain versus bullet formatting. Its
  audit marks caller evidence as unverified and redacts provenance; no Engine construction or
  server-side evidence resolution exists yet.
- [ ] Constrained learned/neural renderer checked against immutable response-plan claims.
- [ ] Vocabulary, morphology, spelling, pronunciation, syntax, semantics, discourse, and
  style curricula.
- [ ] Teacher-OFF retention/generalization benchmarks for paraphrase, ambiguity, repair,
  letter counting, explanation, and varied grounded wording.

## 13. Programming and coding — seeded/acquired over host mechanisms

- [ ] Repository tree, language, manifest, module, file, symbol, type, import, reference,
  dependency, call-edge, test, diagnostic, and ownership concepts.
- [ ] Parser/tree-sitter/compiler adapters producing typed ASTs and source spans.
- [ ] Incremental file fingerprinting and stale-knowledge invalidation.
- [ ] Git status/log/diff/blame/worktree operations with read/mutation effects separated.
- [ ] Compiler, interpreter, formatter, linter, and test-runner capabilities.
- [ ] Package-manager and dependency-audit capabilities with network/mutation separated.
- [ ] Documentation lookup and API/interface discovery capabilities.
- [ ] Safe patch workflow: inspect, hypothesize, patch in scope, targeted checks, evaluate,
  retain/revise, with every mutation receipted.
- [ ] Build/test diagnostic parsing and source linkage.
- [ ] Versioned semantic mapping from Spoon IR/intrinsics/contracts/effects to
  TypeScript, Rust, Python, and other typed AST constructs.
- [ ] IR-to-code lowering with target runtime shims, source maps, declared imports,
  exact dependencies, and reproducible build/test recipes.
- [ ] Code/AST-to-IR lifting with source-span provenance and explicit opaque
  capability boundaries for unrepresentable behavior.
- [ ] Differential equivalence testing across outputs, typed failures, effects,
  and resource envelopes, including semantic false-friend fixtures.
- [ ] Grounded repository response plans whose factual claims cite inspected evidence.
- [ ] Programming-language curricula: expressions, control flow, data structures, types,
  functions, modules, errors, concurrency, I/O, testing, debugging, security, performance.
- [ ] Teacher-OFF acquisition/retention/generalization fixtures across multiple languages
  and toolchains.

## 14. Standard adapters and seed candidates—not new native primitives

These are important capabilities, but should normally be learned/shipped as
typed adapters and procedures over the native families above:

- [ ] Dictionary, spelling, thesaurus, translation, and pronunciation.
- [ ] Search engine, web fetch, robots/policy-aware crawler, and HTML extraction.
- [ ] OpenAPI/JSON Schema/GraphQL/gRPC/CLI-help interface discovery.
- [ ] SQL database, key-value store, vector search, and graph database.
- [ ] Browser automation and accessibility-tree observation.
- [ ] Email, calendar, contacts, messaging, notifications, and task systems.
- [ ] Cloud/object storage and remote repository hosting.
- [ ] PDF/document/spreadsheet/presentation parsing and generation.
- [ ] Image metadata/decoding/OCR, audio transcription, and video inspection.
- [ ] Local/remote model inference as an explicitly bounded proposal capability.
- [ ] Maps/geocoding/weather/finance and other domain APIs with freshness provenance.
- [ ] Hardware/device control adapters with device-specific safety interlocks.

## Completion rule

Checking every row is not the goal. Rows in sections 1–11 define the candidate
native foundation; each must be implemented, deliberately deferred, or explicitly
classified as seeded/acquired with a rationale. Sections 12–14 should grow through
curricula and capability acquisition. A primitive is complete only when its
semantics, limits, errors, contracts, permission behavior, traces/receipts,
serialization, and adversarial tests are all complete.
