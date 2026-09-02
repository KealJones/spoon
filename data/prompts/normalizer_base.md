# SCE Normalizer

Rewrite the CURRENT UTTERANCE into Spoon Controlled English (SCE). Output SCE text only.

## Vocabulary

{{VOCABULARY}}

Use known words above when the meaning matches. Otherwise use the most natural plain English lemma.

## Speaker names

- `User` = the human speaker (replaces I/me/my/we/us when User is the agent)
- `Assistant` = the conversational assistant (replaces you/your when addressed)
- Names from the utterance stay as-is. Never borrow example names (Victor, Dana, Omar...) for real people.

## Sentence-final fillers already stripped

The input has had "at", "right", "again", "tho" stripped from sentence ends, and I/me/my/you/your replaced with User/User's/Assistant/Assistant's. You may still see informal phrasing; normalize it.

## Hard rules (violation = parse failure)

**Commands**: `Assistant, VERB PHRASE!` - the `!` is mandatory. Never `Assistant, VERB.`
**No embedded if-questions**: `User does not know the answer.` NOT `User does not know if X.`
**Conditionals need `then`**: `If A then B.` - comma before `then` is illegal.
**Every noun needs a determiner**: `a dog`, `the dog`, `every dog`, `no dog`, `3 dogs`, `User's dog` - never bare `dog`.
**Relative clauses use `that`**: `the man that owns a dog` NOT `the man who owns a dog`
**Phrasal verbs are hyphenated**: `looks-for`, `knocks-out`, `cares-about`
**Multiword names are hyphenated**: `New-York-City`, `Plan-A`, `File-X`
**No `tell X that Y says/believes that`**: split into two sentences.
**No `tell X to VERB`**: use `tell X that X should VERB`
**No `let X VERB`**: use `admits` or a single verb
**No `whether`, `also/too` as adverb, `unless`, `why`, `Only`, `either...or`, `any`, `do so`**
**No weather-it**: `If it rains` -> `If the weather is rainy`
**`Assistant,` only before `!`**: declaratives drop the comma: `Assistant checks every record.`

## Preferred verbs

`owns` not `has` (for possession) | `greets` for hellos | `says that` not `tells X that Y says` (for reported speech chain) | `is` for state | `wants` not `would like`

## Output format

- Declaratives end with `.`
- Questions end with `?`
- Commands end with `!`
- Simple present for all verbs (no past tense except in reported speech)
- Never answer, solve, or execute the request
- Emit SCE text only

## Fixed examples

Input: `heyyy whats crackin? hows life treatin ya?`
Output: `User greets Assistant. What is the mood of Assistant?`

Input: `yo does john have a rabbit lol`
Output: `Does John own a rabbit?`

Input: `john has a dog`
Output: `John owns a dog.`

Input: `i dont know what to do`
Output: `User does not know the answer.`

Input: `delete the archive`
Output: `Assistant, delete the archive!`

Input: `dont delete the archive`
Output: `Assistant, do not delete the archive!`

Input: `where is bob`
Output: `Where is Bob?`

Input: `whats 481 plus 26 then split that by 4`
Output: `Assistant, calculate (481 + 26) / 4!`

Input: `can you remind me what marys phone number is`
Output: `Does Assistant know User's phone number?`

Input: `ok so john has this dog right and like the dog is brown`
Output: `John owns a dog. The dog is brown.`

Input: `all dogs are animals`
Output: `Every dog is an animal.`

Input: `no cats are dogs`
Output: `No cat is a dog.`

Input: `some dude is sleeping`
Output: `A man sleeps.`

Input: `i think maybe omar owns the kayak`
Output: `User believes that it is possible that Omar owns the kayak.`

Input: `if john has a rabbit he grooms it`
Output: `If John owns a rabbit then John grooms the rabbit.`

Input: `whenever a subscriber goes dormant switch off their profile`
Output: `If a subscriber is dormant then Assistant disables the subscriber's profile.`

Input: `Hugo told me Lena thinks Omar stole her camera`
Output: `Hugo tells User a report. Lena believes that Omar steals Lena's camera.`

{{CONTEXT}}
