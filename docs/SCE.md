# SCE: Spoon Controlled English

SCE is the only language the interior reads. It is a small, unambiguous subset
of English modeled on Attempto Controlled English (ACE 6.7). Messy human input
is rewritten into SCE by the ears (native recognizer first, LLM normalizer on
miss); the SCE parser is the hard gate. Every SCE sentence parses to exactly
one `Clause` (see `crates/spoon-core/src/types/clause.rs`).

Differences from ACE: the lexicon is open (nouns are CAN concepts, verbs are
CAN actions, unknown words are guessed by position and reported), there is an
arithmetic sublanguage, `Should ... ?` is a suggestion question, and the
speaker/addressee are always the fixed names `User` and `Assistant`.

## Tokens

- Words: letters, digits, hyphens, apostrophe for possessive (`Mary's`).
  Multi-word names and phrasal verbs are hyphenated: `New-York-City`,
  `looks-for`, `miles-per-hour`.
- Names: capitalized words not at sentence start, or any word containing a
  hyphen followed by a capital (`Object-X`, `File-A`). `User` and `Assistant`
  are reserved names.
- Variables: single capital letters `X`, `Y`, `Z` optionally followed by digits.
- Numbers: `10`, `3.5`, `-2`, `1e3`. Number words `one`..`twelve`, `dozen` are
  accepted and normalized.
- Strings: double-quoted `"hi there"`. Paths: tokens starting with `/`, `./`,
  `~/`. URLs: `http://...`, `https://...`.
- Sentence terminators: `.` declarative, `!` command, `?` question.

## Sentences

```
Sentence  := Decl '.' | Command '!' | Question '?' | Rule '.'
Decl      := NP VP ( 'and' VP )*
           | 'There is' NP
           | 'There are' NP
Command   := 'Assistant' ',' VPimp ( 'and' VPimp )*
           | 'Assistant' ',' 'do not' VPimp
Rule      := 'If' Decl ( 'and' Decl )* 'then' Decl ( 'and' Decl )*
Question  := 'Does' NP VPbase            -- yes/no
           | 'Is' NP ( Adj | NP | PP )   -- yes/no copula
           | 'Can' NP VPbase | 'Should' NP VPbase | 'Must' NP VPbase | 'May' NP VPbase
           | 'Who' VP                     -- Who owns a dog
           | 'What' VP                    -- What happens
           | 'What is' NP                 -- What is the wellbeing of Assistant
           | 'Which' Noun VP              -- Which person should own the task
           | 'How many' Noun 'does' NP VPbase    -- How many apples does Ben own
           | 'How many' Noun VP           -- How many dogs enter the garden
           | 'Where is' NP | 'When does' NP VPbase
```

## Noun phrases

```
NP   := Det Adj* Noun ( PP )? ( 'that' VP )?
      | Name | Var | String | Number | Path | Url
      | NP "'s" Adj* Noun                -- possessive
      | 'the' Noun 'of' NP               -- property access: the length of X
      | ArithExpr                         -- only as object of calculate/compute
Det  := 'a' | 'an' | 'the' | 'every' | 'no' | 'some'
      | 'not every'
      | 'at least' Number | 'at most' Number | 'exactly' Number | Number
      | 'an unknown'                     -- indefinite with unknown identity
```

Quantifier mapping to `Quant`: `a/an/some` -> Indef, `the` -> Def, `every` ->
Every, `no` -> No, `at least N` -> AtLeast, `at most N` -> AtMost, `exactly N`
-> Exactly, bare `N noun` -> Count(N), Name -> Named, literals -> Literal,
`who/what/which` -> Wh.

Adjectives precede the noun (`a red card`). Adjectives are anything between the
determiner and the noun that is not a known noun; unknown words there are
reported as adjectives.

## Verb phrases

```
VP     := Verb3 NP? PP* ( 'that' Decl )?         -- owns a dog / says that S
        | 'is' Adj PP*                            -- is present / is angry with X
        | 'is' NP                                 -- is a doctor
        | 'is' AdjComp 'than' NP                  -- is smarter than John
        | 'is not' ...                            -- negated copula
        | 'does not' Verbbase NP? PP*
        | Modal ( 'not' )? Verbbase NP? PP*       -- can enter / cannot enter / should log X
        | 'is' Verbpp 'by' NP                     -- passive: is admitted by Mary (rare)
VPimp  := Verbbase NP? PP*  ( 'that' Decl )?      -- close the door / tell X that S
VPbase := Verbbase NP? PP*
Modal  := 'can' | 'cannot' | 'should' | 'must' | 'may'
PP     := Prep NP
Prep   := 'to' | 'at' | 'in' | 'on' | 'from' | 'with' | 'for' | 'about' | 'toward'
        | 'into' | 'of' | 'by' | 'as' | 'before' | 'after' | 'than'
```

Verbs appear third-person singular in declaratives (`owns`, `says`) and base
form after `Assistant,`, `does not`, modals, and in questions. The parser
lemmatizes both to the base form; `Pred.pred` is always the base lemma
(`own`, `say`). Hyphenated phrasal verbs lemmatize the first part
(`looks-for` -> `look-for`).

Copula: `X is Adj` -> `Pred { pred: "be", args: [X], attr: Some(adj) }`.
`X is a Noun` -> `Pred { pred: "be", args: [X, y] }` with `y` an Indef
referent of that noun (an is-a assertion). `X is Adj-er than Y` ->
`Pred { pred: "be", args: [X, Y], attr: Some("smarter-than") }`.

Embedded clauses: `X says that S` -> `Pred { pred: "say", args: [X, Term::Sub(S)] }`.
Verbs that embed: say, believe, think, know, report, want, tell (as
`tell Y that S` -> args [X, Y, Sub]), ask, mean.

## Arithmetic

Only as the object of `calculate`, `compute`, or `evaluate`, or on the right of
`is`:

```
ArithExpr := Term (('+'|'-') Term)*
Term      := Factor (('*'|'/'|'%') Factor)*
Factor    := Unary ('^' Unary)?
Unary     := '-' Unary | Atom
Atom      := Number | Var | '(' ArithExpr ')'
```

`Assistant, calculate (3 / 500) * 3600!` -> Command with
`Pred { pred: "calculate", args: [Term::Arith(...)] }`.

## Fixed names and reserved words

- `User` is the human. `Assistant` is Spoon. Never `I`, `you`, `me`.
- `now`, `today`, `tomorrow`, `yesterday` are DateTime literals.
- No pronouns except in the antecedent-safe pattern `X ... X` with variables.
  The normalizer replaces `it/that/this/he/she/they` with a name, a variable,
  or a definite NP.

## Referent rules (from the ACE spike, empirically true)

- Commands, questions, negation, modals, and `says that` are islands: an
  indefinite noun born inside one is a new object in the next sentence.
- To share an object across sentences use a Name (`Object-X`, `File-A`), a
  variable introduced at top level (`There is a file X. Assistant, open X!`),
  or a definite NP that the discourse module will resolve.
- `if` exports referents into `then` only.

## Examples (all must parse)

```
User greets Assistant.
What is the wellbeing of Assistant?
Assistant, calculate 2 * (((599 + 32) / 0) * 6)!
Not every dog likes a cat.
If a customer owns a card then the machine accepts the card.
John says that Mary believes that Bob owns the dog.
User does not know that the statement is true.
Who is John?
Assistant, tell Mike that User refuses!
There is a file X. Assistant, open X! Assistant, save X!
Should Assistant update Object-X to "hi"? Should Assistant save Object-X?
Ben owns 10 apples. A train travels toward Ben. The train moves at 500 miles-per-hour.
Every customer that is not an admin owns a card.
No banned customer can enter.
Mary is smarter than John.
Bob may enter.
How many apples does Ben own?
Which person should own the task?
Assistant, do not answer the question!
An unknown woman says that an unknown man tells a woman that the unknown woman is angry with a man.
```

## Realization (Clause -> SCE)

`spoon_lang::sce::realize(&Clause) -> String` must produce a sentence that
re-parses to an equal `Clause` (modulo variable names). This round trip is the
parser's main test and is what the phrasing learner stores.
