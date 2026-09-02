# Spoon Architecture - Concrete Algorithm Design Memo

---

## 1. Lexicon/Phrasing Induction from (Messy, SCE) Pairs

**Prior art:**
- IBM Model 1 / GIZA++ word alignment: https://aclanthology.org/J93-2003.pdf
- Moses phrase extraction: https://aclanthology.org/N03-1017.pdf  
- GENLEX (Zettlemoyer & Collins): https://aclanthology.org/W05-0602.pdf
- SippyCup/SEMPRE: https://nlp.stanford.edu/software/sempre/
- Template induction: Kwiatkowski et al. EMNLP 2010

**Practical pick for ~5k pairs, no GPU:** Phrase-table extraction + slotted templates + BM25 fallback. Skip GENLEX (requires full CCG grammar and chart parser). Skip SEMPRE (huge deps). Do this:

**Pipeline:**

```
Step 1: Slot extraction
  For each pair (messy, sce):
    shared_values = extract_shared_tokens(messy, sce)
      // numbers, names (capitalized), quoted strings, file paths
    messy_template = replace(messy, shared_values, "<SLOT_i>")
    sce_template   = replace(sce,   shared_values, "<SLOT_i>")
    store (messy_template, sce_template, slot_types)

Step 2: IBM Model 1 word alignment (per non-slot token)
  For each (messy_toks, sce_toks) with slots removed:
    Initialize t(e|f) = 1/|vocab_e| for all e in sce_toks, f in messy_toks
    For 5 EM iterations:
      counts = {}
      For each (f, e) pair:
        denom = sum_{e'} t(e'|f)
        delta = t(e|f) / denom
        counts[(e,f)] += delta * pair_weight
      t(e|f) = counts[(e,f)] / sum_{e'} counts[(e',f)]
    // Result: soft alignment between messy and SCE tokens

Step 3: Template store
  TemplateEntry {
    messy_pattern: Vec<Token>,  // mix of literals + SLOT placeholders
    sce_pattern:   Vec<Token>,
    slot_map:      Vec<(usize, usize, SlotType)>,  // messy_slot_i -> sce_slot_j
    freq:          u32,
    example_pairs: SmallVec<[usize; 5]>,
  }
  store as HashMap<TemplateFingerprint, Vec<TemplateEntry>>
  fingerprint = sorted set of non-slot content words (for fast lookup bucket)

Step 4: Matching new input `q`
  candidates = []

  // Exact match after slot extraction
  q_slots = extract_values(q)
  q_template = replace(q, q_slots, "<SLOT_i>")
  if q_template in store: return best_entry, fill slots -> score 1.0

  // Slotted fuzzy match
  q_fp = content_words(q_template)
  for entry in store where jaccard(q_fp, entry.fingerprint) > 0.3:
    score = bm25(q_template, entry.messy_pattern)
           + 0.3 * slot_type_compat(q_slots, entry.slot_map)
    candidates.push((entry, score))

  // Edit distance fallback (only for short inputs <8 tokens)
  if candidates empty or top_score < 0.5:
    for entry in all_entries:
      ed = normalized_edit_distance(q_template, entry.messy_pattern)
      candidates.push((entry, 1 - ed))

  sort candidates by score desc
  if top_score > THRESHOLD_HIGH (0.75): return top, fill slots
  if top_score > THRESHOLD_LOW  (0.45): return top with low_confidence flag
  else: fallback to LLM codec, add result as new training pair
```

**Key data structures:** Inverted index from content words -> template IDs for fast BM25 candidate retrieval. Keep a `LexiconGrowthQueue` of LLM-codec outputs pending review; batch retrain alignment monthly.

**Pitfalls:** Slot alignment fails when a value appears only on one side (e.g., "thanks" -> nothing). Guard: if shared_value count == 0, treat entire utterance as unslotted template. Over-generalization: "add <SLOT_0>" matches "add butter to the pan" when you want arithmetic. Add slot type constraints (NUMBER vs NOUN_PHRASE) to the TemplateEntry.

**Skip:** Neural paraphrase models, CCG induction, any beam search over derivations.

---

## 2. Typo/Slang Normalization

**Prior art:** SymSpell by Wolf Garbe: https://github.com/wolfgarbe/SymSpell — O(1) lookup via delete-neighborhood.

**Algorithm:**

```
Build phase (run once, rebuild incrementally when lexicon changes):
  delete_dict: HashMap<String, Vec<String>>
  for word in lexicon:
    for delete_variant in all_deletes(word, max_edit=2):
      delete_dict[delete_variant].push(word)

  // all_deletes: generate all strings reachable by deleting 1..max_edit chars
  // For 2-edit, that's O(len^2) variants per word - fine for <200k words

Lookup(input, verbosity: Closest|All|Top):
  candidates: HashMap<String, (String, u32)>  // variant -> (original, edit_dist)
  input_deletes = all_deletes(input, max_edit=2)
  candidates.insert(input, (input, 0))  // exact match first
  for d in input_deletes:
    if d in delete_dict:
      for candidate in delete_dict[d]:
        dist = damerau_levenshtein(input, candidate)
        if dist <= 2: candidates.insert(candidate, (candidate, dist))
  // Sort by dist ASC, then by frequency DESC

Runtime lexicon changes:
  on add_word(w):
    for d in all_deletes(w, 2): delete_dict[d].push(w)
  on remove_word(w):
    // Mark as tombstone; don't bother removing all delete variants
    tombstones.insert(w)

Slang table (apply BEFORE SymSpell):
  static SLANG: HashMap<&str, &str> = {
    "u" -> "you", "ur" -> "your", "gonna" -> "going to",
    "lemme" -> "let me", "wanna" -> "want to", "wasup" -> "what is up",
    "thx" -> "thanks", "pls" -> "please", "tmrw" -> "tomorrow",
    // ~200 entries covers 90% of SMS slang
  }
  for token in tokenize(input):
    if token.is_lowercase() && len < 6 && token in SLANG:
      replace

Protect proper names and code:
  if token starts with uppercase AND in entity_registry: skip
  if token matches path_regex (contains '/' or '\' or '.'): skip
  if token in backtick_span or code_fence: skip
```

**Pitfall:** SymSpell will "correct" short words aggressively - "a" -> "at", "i" -> "in". Add minimum length guard (skip normalization for len <= 2) and require frequency(candidate) > K*frequency(input) to apply correction.

---

## 3. Value Spotting (Duckling-style)

**Rust crates:** `chrono` + `chrono-english` (https://crates.io/crates/chrono-english) for date/time; `evalexpr` (https://crates.io/crates/evalexpr) for arithmetic; `meval` (older, less maintained); `regex` for structural patterns; `url` crate for URL validation.

**Architecture:** Ordered regex + rule pipeline producing `Span { start, end, value: TypedValue, confidence }`. Then resolve overlaps via priority + coverage.

```rust
enum TypedValue {
    Integer(i64), Float(f64), Bool(bool),
    DateTime(chrono::DateTime<Utc>), Duration(chrono::Duration),
    Text(String), FilePath(PathBuf), Url(url::Url),
    Json(serde_json::Value), Email(String),
    Expression(f64),  // evaluated inline arithmetic
}

fn spot_values(text: &str, now: DateTime<Utc>) -> Vec<Span> {
    let mut spans = vec![];

    // 1. Quoted strings (highest priority - grab before anything else)
    for m in QUOTED_RE.find_iter(text):
        spans.push(Span { value: Text(m.inner()), confidence: 1.0, .. })

    // 2. URLs (before general tokens eat them)
    for m in URL_RE.find_iter(text): ...

    // 3. File paths  (/foo/bar, C:\foo, ./foo, ~/foo)
    for m in PATH_RE.find_iter(text): ...

    // 4. Emails
    for m in EMAIL_RE.find_iter(text): ...

    // 5. JSON blobs  (heuristic: starts with { or [, valid parse)
    for m in JSON_CANDIDATE_RE.find_iter(text):
        if let Ok(v) = serde_json::from_str(m.as_str()): spans.push(...)

    // 6. Inline arithmetic expressions
    //    Pattern: digits + ops +  digits (possibly with words "times", "divided by")
    //    Normalize: "times"->"*", "divided by"->"/", "plus"->"+"  etc.
    //    Then: evalexpr::eval_number(normalized) -> f64
    for m in ARITH_PHRASE_RE.find_iter(text):
        let normalized = normalize_arith_words(m.as_str());
        if let Ok(n) = evalexpr::eval_number(&normalized):
            spans.push(Span { value: Expression(n), ... })

    // 7. Date/time (chrono-english or hand-rolled)
    //    "tomorrow at 3pm", "next friday", "in 2 hours", "last tuesday"
    for m in DATETIME_PHRASE_RE.find_iter(text):
        if let Some(dt) = chrono_english::parse_date_string(m.as_str(), now, ..):
            spans.push(...)

    // 8. Word-numbers ("twice"->2, "a dozen"->12, "half"->0.5, "thrice"->3)
    for m in WORD_NUM_RE.find_iter(text):
        spans.push(Span { value: Integer(WORD_NUM_MAP[m.as_str()]), ... })

    // 9. Bare integers / floats / percentages
    for m in NUMBER_RE.find_iter(text): ...

    // 10. Overlap resolution
    resolve_overlaps(&mut spans)  // keep highest confidence, prefer longer span
    spans
}

fn resolve_overlaps(spans: &mut Vec<Span>) {
    // Sort by start, then by length desc, then by confidence desc
    // Greedy scan: if span[i] overlaps span[j] (j > i):
    //   keep the one with higher confidence; if equal, keep longer
    spans.sort_by(|a, b| a.start.cmp(&b.start)
        .then(b.len().cmp(&a.len()))
        .then(b.confidence.partial_cmp(&a.confidence).unwrap()));
    let mut kept = vec![];
    let mut end = 0;
    for s in spans.drain(..):
        if s.start >= end: kept.push(s); end = s.end;
    *spans = kept;
}
```

**Pitfall:** Arithmetic `evalexpr` will parse "3 apples" as `3` - only apply inside matched arithmetic phrases. Division-by-zero: catch and emit a special `DivByZero` value (the user asked "divide by 0" intentionally in your example - handle gracefully, return `Infinity` or an error value the planner can act on).

---

## 4. Planner Over Typed Action Graph (AND/OR Hypergraph)

**Prior art:**
- Knuth's algorithm for AND/OR graphs: Knuth 1977 "A Generalization of Dijkstra's Algorithm"
- Gallo et al. shortest hyperpaths: https://doi.org/10.1016/0166-218X(93)90045-P
- Bixby capsule planning: https://bixbydevelopers.com/dev/docs/dev-guide/developers/actions
- STRIPS regression planning: Russell & Norvig Ch. 10

**Data model:**

```rust
struct Action {
    id: ActionId,
    inputs:  Vec<InputSlot>,  // each has: type, Required/Optional, One/Many
    output:  TypeSlot,        // typed output
    cost:    f32,             // lower = preferred; popularity-adjusted at runtime
}
struct TypeEdge { from: TypeId, to: TypeId, kind: IsA | HasA(field) | RoleOf }
struct Signal { name: String, value: TypedValue, type_id: TypeId }
```

**AND/OR graph:** Each action node is an AND-node (ALL required inputs must be satisfied). Type nodes are OR-nodes (ANY producer action satisfies them). A signal satisfies its type directly.

```
fn plan(goal_type: TypeId, signals: Vec<Signal>, actions: &ActionGraph) 
    -> Option<Plan>
{
    // Knuth/Dijkstra on AND/OR hypergraph
    // dist[node] = best cost to PRODUCE this type (OR-node) or satisfy this action (AND-node)
    
    dist: HashMap<NodeId, f32> = {}
    pred: HashMap<NodeId, NodeId> = {}
    pq:   BinaryHeap<(OrderedF32, NodeId)>  // min-heap

    // Initialize with signals (cost 0)
    for s in signals:
        dist[type_node(s.type_id)] = 0.0
        pq.push((0.0, type_node(s.type_id)))
    // Also propagate up type hierarchy (IsA edges, cost 0)
    for (sub, sup) in type_edges where kind==IsA:
        if dist[type_node(sub)] is known:
            relax(type_node(sup), dist[type_node(sub)])

    while let Some((d, node)) = pq.pop():
        if d > dist[node]: continue  // stale

        if node == type_node(goal_type): 
            return Some(reconstruct_plan(pred, goal_type))

        if node is TypeNode(t):
            // This type is now satisfied; notify all actions that need t
            for action in actions_consuming(t):
                maybe_satisfy_action(action, &mut dist, &mut pq, &pred)

    // goal unreachable: find unsatisfied Required inputs -> return Plan with placeholders
    return Some(build_partial_plan_with_prompts(goal_type, dist))
}

fn maybe_satisfy_action(a: &Action, dist, pq, pred):
    // AND-node: action is ready when ALL required inputs are satisfied
    required_costs = []
    for slot in a.inputs where slot.cardinality == Required:
        if let Some(c) = dist.get(type_node(slot.type_id)):
            // If Many cardinality: can use a single value or map over a list
            required_costs.push(c)
        else:
            return  // not yet satisfiable
    action_cost = a.cost + sum(required_costs)
    // optional inputs add bonus (lower cost) if available
    for slot in a.inputs where slot.cardinality == Optional:
        if dist.contains(type_node(slot.type_id)):
            action_cost -= 0.1  // small bonus for richer output
    if action_cost < dist.get(action_node(a.id)).unwrap_or(INF):
        dist[action_node(a.id)] = action_cost
        pred[action_node(a.id)] = best_input_combination
        pq.push((action_cost, action_node(a.id)))
        // Now the action's output type might be cheaper
        out_cost = action_cost + 0.0  // action node -> output type
        if out_cost < dist.get(type_node(a.output)).unwrap_or(INF):
            dist[type_node(a.output)] = out_cost
            pred[type_node(a.output)] = action_node(a.id)
            pq.push((out_cost, type_node(a.output)))
```

**One vs Many:** When a slot is `Many`, check if `List<T>` is available (from a prior action returning a list) OR insert a `map(source: T, fn: T->U) -> List<U>` combinator action automatically. Track list types as `ListOf(TypeId)` in the type graph.

**Plan scoring for parse-ranking:** `score = -plan_length * 0.3 - total_cost + popularity_bonus` where `popularity_bonus = sum over actions of log(use_count + 1)`. Use this to rank when multiple valid plans exist.

**Pitfall:** Cycles in type hierarchy (IsA loops). Detect during graph construction, not at query time. Also: action with output type T that also consumes T (self-referential) - add a cycle guard (don't re-enqueue a type node if action that produces it already consumed it in this plan).

---

## 5. Budgeted Enumerative Program Synthesis with OE Pruning

**Prior art:**
- BUSTLE: https://arxiv.org/abs/2007.14381  
- Probe: https://arxiv.org/abs/2109.11977
- DreamCoder: https://arxiv.org/abs/2006.08381
- Escher: https://dl.acm.org/doi/10.1145/2737924.2737977
- Transit (Google Sheets): https://dl.acm.org/doi/10.1145/3173574.3173860

**Core algorithm:**

```
// Per-type banks: bank[type][size] = list of programs of that type with that cost
type Banks = HashMap<TypeId, BTreeMap<u32, Vec<Program>>>;

fn synthesize(
    examples: Vec<(Input, Output)>,  // I/O spec from teacher LLM
    grammar:  &Grammar,              // primitives + learned library
    budget:   Budget,                // max_nodes: usize, max_ms: u64
) -> Option<Program>
{
    let start = Instant::now();
    let mut banks: Banks = HashMap::new();
    
    // Seed banks with constants from utterance + small pool (0, 1, "", [], true, false)
    for c in extract_constants_from_utterance() + CONSTANT_POOL:
        if satisfies_examples(&Program::Const(c), &examples):
            return Some(Program::Const(c))
        banks[type_of(c)][1].push(Program::Const(c))

    // BFS by cost (not depth!)
    let mut cost = 2u32;
    while cost <= budget.max_cost:
        if start.elapsed() > budget.max_ms: break;
        
        for primitive in grammar.primitives:  // includes library abstractions
            // Enumerate all ways to fill primitive's arity with programs
            // from banks whose costs sum to (cost - prim_cost)
            for arg_combo in cost_splits(primitive.arity, cost - primitive.cost, &banks):
                let prog = Program::App(primitive, arg_combo);
                
                // OE check: compute output on all example inputs
                let obs = evaluate_on_all(&prog, &examples);
                if obs is Error: continue;
                if obs_signature(&obs) in seen_signatures[type_of(prog)]: continue;  // OE prune
                seen_signatures[type_of(prog)].insert(obs_signature(&obs));
                
                if obs == expected_outputs(&examples):
                    return Some(prog)
                
                if banks[type_of(prog)][cost] not over capacity(50):
                    banks[type_of(prog)][cost].push(prog)
        
        // Higher-order: map/filter/fold with bounded lambda bodies
        for hof in [Map, Filter, Fold]:
            for lambda_cost in 1..=(cost - hof.base_cost - 1):
                for lambda in enumerate_lambdas(lambda_cost, &banks):
                    let prog = Program::HOF(hof, lambda);
                    // same OE check...
        
        cost += 1;
    None
}

fn obs_signature(outputs: &[Value]) -> u64 {
    // Hash all outputs together; use xxhash for speed
    let mut h = XxHash64::default();
    for o in outputs: o.hash(&mut h);
    h.finish()
}
```

**Probe-style priors:** Maintain `P(primitive | context_type)` as a frequency table over successful syntheses. Use this to sort `grammar.primitives` before inner loop - high-probability primitives first. Update after each solved problem. This gives 3-5x speedup without any neural components.

**Realistic limits (laptop, ~150 primitives, 5 I/O examples):**
- Cost budget 6, 5 examples with small values: ~10M programs enumerated, ~5s
- Cost budget 8: ~500M, likely infeasible without aggressive OE pruning
- Keep DSL small: 50-70 "core" primitives, rest in learned library

**Pitfall:** HOF explosion - `map(filter(map(...)))` generates huge search space. Hard cap: lambda body depth <= 3. Also cap bank size per type per cost at 50-100 programs (keep by OE-distinctness, not order).

---

## 6. Library Consolidation (Stitch-style)

**Prior art:**
- Stitch: https://arxiv.org/abs/2211.16605 (GitHub: https://github.com/mlb2251/stitch)
- DreamCoder abstraction: https://arxiv.org/abs/2006.08381
- Anti-unification: Plotkin 1970, Reynolds 1970; practical: Bulychev & Minea WCRE 2008

**Simplified practical version (no Stitch dependency, pure Rust):**

```
// Sleep phase: run after ~100 new programs added to library

fn consolidate(corpus: &[Program]) -> Vec<Abstraction> {
    // Step 1: Hash all subtrees to find frequent shapes
    let mut subtree_hash_counts: HashMap<u64, (SubtreeShape, u32)> = {};
    for prog in corpus:
        for subtree in all_subtrees(prog):  // DFS, excluding leaves
            let h = structural_hash(&subtree);  // hash ignoring constants
            subtree_hash_counts[h].1 += 1;
    
    // Step 2: Candidate abstractions: subtrees appearing >= MIN_FREQ times
    let candidates: Vec<SubtreeShape> = subtree_hash_counts
        .into_values()
        .filter(|(_, count)| *count >= MIN_FREQ)  // MIN_FREQ = 3
        .map(|(shape, _)| shape)
        .collect();
    
    // Step 3: Anti-unify candidates to introduce parameters
    // Anti-unify the actual subtrees (not just shapes) to find the most specific 
    // common structure with variables for differing parts
    let mut abstractions = vec![];
    for shape in candidates:
        let instances: Vec<Program> = corpus.iter()
            .flat_map(|p| matching_subtrees(p, &shape))
            .collect();
        if instances.len() < MIN_FREQ: continue;
        
        let anti_unified = anti_unify_all(&instances);
        // anti_unified is a program with some leaves replaced by Var(i)
        
        // Step 4: Compute utility
        let cost_before = instances.iter().map(|p| cost(p)).sum::<f32>();
        let abstract_call_cost = 1.0 + anti_unified.num_vars() as f32 * 0.5;
        let cost_after = instances.len() as f32 * abstract_call_cost;
        let utility = cost_before - cost_after - cost_of_definition(&anti_unified);
        
        if utility > 0.0:
            abstractions.push(Abstraction { 
                body: anti_unified, 
                utility,
                // Name from phrasings: take most common SCE verb+noun from programs 
                // that reference these instances
                suggested_name: name_from_phrasing(&instances, corpus),
            })
    
    // Step 5: Greedy non-overlapping selection
    abstractions.sort_by(|a, b| b.utility.partial_cmp(&a.utility).unwrap());
    let selected = greedy_non_overlapping(abstractions);
    
    // Step 6: Evict single-use abstractions on next consolidation pass
    for abs in library where abs.use_count < 2 && abs.age > EVICTION_AGE:
        library.remove(abs)
    
    selected
}

fn anti_unify(p1: &Program, p2: &Program) -> Program {
    match (p1, p2):
        (App(f1, args1), App(f2, args2)) if f1 == f2 && args1.len() == args2.len():
            App(f1, zip(args1, args2).map(|(a, b)| anti_unify(a, b)).collect())
        (Const(a), Const(b)) if a == b: Const(a)
        _: Var(fresh_var())  // differ here; introduce parameter
}
```

**Naming abstractions:** After selecting an abstraction, look at which corpus programs use its instances. Pull the SCE phrasings associated with those programs. Extract the main verb + object noun via POS tags (simple regex: first capitalized word = verb, first lowercase noun after verb = object). That becomes the abstraction name.

**When to run:** After every 100 new programs, or during an explicit "sleep phase" triggered by low activity. Don't run during active dialog.

---

## 7. Dialog State and Memory

**Prior art:**
- Information State Update / TrindiKit: Larsson & Traum 2000 https://aclanthology.org/W00-0302.pdf
- Centering Theory: Grosz, Joshi & Weinstein 1995
- ACT-R base-level activation: Anderson et al. 2004 https://doi.org/10.1007/978-3-540-24622-4_3
- Hobbs reference resolution: Hobbs 1978

**Information State:**

```rust
struct DialogState {
    qud:         Vec<Question>,         // Questions Under Discussion stack
    commitments: HashMap<PropId, Prop>, // shared facts both parties accept
    agenda:      Vec<DialogMove>,       // pending grounding moves
    entities:    Vec<EntityRecord>,     // recently mentioned, recency-ranked
    episodes:    Vec<Episode>,          // longer-term memory
    correction_pending: Option<CorrectionFrame>,
}

struct EntityRecord {
    entity_id:  EntityId,
    type_id:    TypeId,
    last_mention_turn: u32,
    mention_count: u32,
    surface_forms: Vec<String>,  // "the door", "it", "that one"
}

// ACT-R base-level activation
fn activation(ep: &Episode, now: f32, d: f32 = 0.5) -> f32 {
    // ep.access_times: Vec<f32>  (timestamps when episode was retrieved)
    let sum: f32 = ep.access_times.iter()
        .map(|&t| (now - t).powf(-d))
        .sum();
    sum.ln()  // B_i = ln(sum t_j^{-d})
}

// Referent resolution (Hobbs-lite + Centering)
fn resolve_referent(np: &NP, state: &DialogState) -> Option<EntityId> {
    let candidates: Vec<&EntityRecord> = state.entities.iter()
        .filter(|e| type_compatible(e.type_id, np.semantic_type))
        .collect();
    
    match np.form:
        Pronoun("it") | Pronoun("that") => {
            // Most recent entity of compatible type (Centering: Cb = backward-looking center)
            candidates.iter()
                .max_by_key(|e| e.last_mention_turn)
                .map(|e| e.entity_id)
        }
        DefiniteNP(head) => {
            // Check explicit antecedent first, then recency
            candidates.iter()
                .find(|e| e.surface_forms.contains(&head))
                .or_else(|| candidates.iter().max_by_key(|e| e.last_mention_turn))
                .map(|e| e.entity_id)
        }
        _ => None
    }
}

// Correction detection
fn detect_correction(utterance: &[Clause]) -> Option<CorrectionFrame> {
    let correction_triggers = ["no", "wrong", "not that", "i meant", "actually", 
                                "wait", "never mind", "cancel that"];
    if correction_triggers.iter().any(|t| utterance_contains(utterance, t)):
        Some(CorrectionFrame {
            negated_prop: last_committed_action(),
            clarification_needed: true,
        })
    else: None
}

// Deep recall gate (cheap check before activating episode retrieval)
fn needs_deep_recall(utterance: &[Clause], state: &DialogState) -> bool {
    // Temporal phrases: "last time", "yesterday", "when we"
    has_temporal_phrase(utterance)
    // Definite NP with no live antecedent in entities
    || definite_nps(utterance).any(|np| resolve_referent(np, state).is_none())
    // Explicit memory trigger: "remember", "recall", "you told me"
    || contains_memory_verb(utterance)
}

// Teaching handler: "X means Y", "when I say X do Y", "call it X"
fn handle_teaching(clause: &TeachingClause, state: &mut DialogState) {
    match clause:
        SynonymTeaching(phrase, sce) => lexicon.add_pair(phrase, sce),
        ActionTeaching(trigger, action) => {
            // Add to CAN as a new action or update existing
            can.add_user_defined_action(trigger, action)
        }
        NameTeaching(entity_id, name) => {
            state.entities.get_mut(entity_id).surface_forms.push(name)
        }
}
```

**QUD handling:** When planner has unsatisfied Required inputs, push them as Questions onto QUD. Next user turn: try to resolve top-of-QUD question first before re-planning. Pop on resolution.

---

## 8. Prior Art Worth Stealing From

| System | One-line relevance |
|---|---|
| **SHRDLU** (Winograd 1972) | Proof that a closed-world, typed-object dialog with planner is buildable - your architecture is essentially a modern SHRDLU. Read the thesis. |
| **Cyc** | Avoid its ontology sprawl, but steal the distinction between *isa* (instance) vs *genls* (subset) for your type graph edges. |
| **ACT-R** | Base-level activation formula is directly usable (section 7). Procedural/declarative memory split maps cleanly to your action library vs episode store. |
| **SOAR** | Chunking (learn new rules from successful problem-solving traces) is exactly what your library consolidation + synthesis pipeline does - read Laird's chunking formalism for framing. |
| **Semantic Machines dataflow graphs** (Andreas et al. 2020, https://arxiv.org/abs/2009.11423) | Their "computation as deferred execution with revisions" is exactly your dialog-correction model. The revision operators (delete, recompute) map cleanly to your QUD correction frame. |
| **Bixby selection learning** | They use a ranker trained on user selection signals to pick among multiple valid plans. You can do this with a simple logistic regression on (plan features -> accepted/rejected). |
| **Rasa / Snips NLU** | Snips' slot-filling intent model is much simpler and more auditable than Rasa's; their entity extraction is basically a CRF over IOB tags - practical for you as a fallback after lexicon miss. |
| **AIML/ChatScript** | Pattern-matching dialog is a degenerate case of your template store - AIML's `<srai>` (recursive rewrite) is useful framing for your LLM-codec fallback loop. |
| **Adapt (Mycroft)** | Keyword + regex intent parser; directly ports to Rust in ~200 lines. Good initial fallback before your lexicon is trained. GitHub: https://github.com/MycroftAI/adapt |
| **Padatious** | Neural intent detection; skip entirely - you explicitly want no GPU interior. |
| **OpenCog** | ECAN attention allocation (importance spreading on a hypergraph) is interesting for prioritizing which concepts to expand in your type graph - but too complex for v1. Revisit later. |

---

## Summary Priorities

Build in this order (each unblocks the next):

1. **Value spotter** (regex + evalexpr + chrono) - needed by everything
2. **SymSpell** + slang table - gates lexicon quality
3. **Template store** with BM25 matching - immediate SCE coverage without LLM
4. **AND/OR planner** (Knuth Dijkstra on type hypergraph) - core execution
5. **IBM Model 1 alignment** + lexicon growth loop - grow away from LLM
6. **Dialog state** (ISU + referent resolution) - multi-turn coherence
7. **Synthesis + consolidation** - runs async in sleep phase, not on critical path

The LLM codec stays on the critical path only until your template store + Model 1 lexicon covers >80% of inputs; at that point it becomes a rarely-invoked fallback.