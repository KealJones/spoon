# Attributions

This file records the source and license for every third-party or attributed
dataset used in the Spoon project.

---

## ekg-ai seed brain and curriculum

**Files derived from:**
- `data/seed/lexicon.json` (verbs, nouns partially)
- `data/seed/facts_k8.json` (converted from `curriculum-k8.json`)
- `data/bench/babi_probes.json` (probe families informed by `EKGBENCH.md`)

**Source:** ekg-ai by Keal Jones
https://github.com/kealjones/ekg-ai

**License:** MIT (personal project)

**Attribution:** The seed brain lexemes, K-8 curriculum facts, and bAbI
benchmark taxonomy (`EKGBENCH.md`) are derived from ekg-ai, a prior personal
project by the same author. Used with permission.

---

## WordNet

**Files derived from:**
- `data/seed/lexicon.json` (noun hypernyms, verb synonyms, adjective list)

**Source:** WordNet 3.1 subset via npm:wordnet, curated by ekg-ai project
(`ekg-data/wordnet-curated.json`)

**Original source:** Princeton University WordNet 3.0
https://wordnet.princeton.edu

**License:** WordNet License (Princeton University)
https://wordnet.princeton.edu/license-and-commercial-use

**Attribution:** WordNet 3.1 Copyright 2006 by Princeton University. All rights
reserved. THIS SOFTWARE AND DATABASE IS PROVIDED "AS IS" AND PRINCETON UNIVERSITY
MAKES NO REPRESENTATIONS OR WARRANTIES, EXPRESS OR IMPLIED. By using this file
you agree to comply with the WordNet License.

---

## bAbI Task Taxonomy

**Files derived from:**
- `data/bench/babi_probes.json` (family taxonomy only; probe stories are
  original SCE sentences, not copied from bAbI)

**Source:** Weston et al., "Towards AI-Complete Question Answering: A Set of
Prerequisite Toy Tasks" (2015), arXiv:1502.05698
https://arxiv.org/abs/1502.05698

**License:** BSD License (Facebook AI Research)

**Attribution:** The 20-task family taxonomy is from the bAbI project by
Facebook AI Research. The probe sentences in `babi_probes.json` are original
SCE examples inspired by the task definitions, not reproduced from the bAbI
dataset. The task names and family numbering follow the original paper.

---

## ACE Normalizer Spike

**Files derived from:**
- `data/prompts/normalizer_base.md` (structure, rules, and hard-won constraints)

**Source:** ACE normalizer spike by Keal Jones
`/Users/kealjones/Downloads/ace_normalizer_spike/`

**License:** Personal project by Keal Jones; all rights retained by author.

**Attribution:** The normalizer system prompt (`normalizer_base.md`) adapts
the structure, grammar rules, island rules, and example patterns from
`ACE_NORMALIZER_PROMPT.md`, which scored 116/152 on the ace normalization
test corpus using qwen3.5:4b. The target language has been changed from
ACE 6.7 to SCE (Spoon Controlled English). No test corpus inputs or
expected outputs are included in the prompt (leak check: 0/158).

---

## Slang and Dialog Phrasings

**Files:**
- `data/seed/slang.json`
- `data/seed/dialog_phrasings.json`

**Source:** Original compilation by this agent drawing on general knowledge
of English texting conventions, informal speech patterns, and common
conversational moves.

**License:** Part of the Spoon project; see project root LICENSE.

---

## Summary Table

| File | Primary source | License |
|---|---|---|
| `data/seed/lexicon.json` | ekg-ai, WordNet 3.1, common English | MIT + WordNet License |
| `data/seed/slang.json` | Original | Spoon project |
| `data/seed/dialog_phrasings.json` | Original | Spoon project |
| `data/seed/facts_k8.json` | ekg-ai curriculum-k8.json | MIT |
| `data/bench/babi_probes.json` | bAbI taxonomy (stories: original) | BSD (taxonomy) |
| `data/prompts/normalizer_base.md` | ACE normalizer spike | Personal (Keal Jones) |
| `data/seed/common_words.txt` | Hand-compiled common-English word list | Hand-compiled, no external source |
