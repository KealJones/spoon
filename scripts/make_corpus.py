#!/usr/bin/env python3
"""Generate the graded bench corpora.

Every case carries the answer, so a run says whether Spoon was right rather
than only which path it took. The answer is the rendered result concept, not
the reply: the mouth is a language model, so grading its prose would measure
the mouth's mood.

Surface variation is the point. A corpus where every case is spelled the same
way trains the ears to recognize one template, which is not the problem. So
each intent is asked several ways, including the way people actually type:
lowercase, no punctuation, typos, filler, contractions.
"""

import json
import os
import pathlib
import random

random.seed(7)

# How many of each intent to generate. Bigger is not automatically better: a
# run costs a few seconds per case, and the surface forms repeat before the
# count does, so past a point you are paying for duplicates that get
# deduplicated anyway. Crank it when you have the hours.
SCALE = float(os.environ.get("SCALE", "3"))


def many(base):
    return max(1, int(base * SCALE))  # Reproducible: a corpus that shuffles is a corpus you cannot bisect.

OUT = pathlib.Path(__file__).resolve().parent.parent / "data" / "bench"

WORDS = [
    "strawberry", "mississippi", "banana", "science", "kubernetes", "spoon",
    "concept", "realization", "hello", "coffee", "keyboard", "bookkeeper",
    "assessment", "possession", "committee", "parallel", "recommend",
]


def typo(s):
    """One plausible transposition. How people actually type."""
    if len(s) < 4:
        return s
    i = random.randrange(1, len(s) - 2)
    return s[:i] + s[i + 1] + s[i] + s[i + 2:]


def case(say, expect, category):
    return {"say": say, "expect": expect, "category": category}


def arithmetic():
    out = []
    ops = [
        ("plus", "+", lambda a, b: a + b),
        ("minus", "-", lambda a, b: a - b),
        ("times", "*", lambda a, b: a * b),
    ]
    for name, sym, f in ops:
        for _ in range(many(40)):
            a, b = random.randint(2, 400), random.randint(2, 200)
            want = str(f(a, b))
            forms = [
                f"what is {a} {name} {b}",
                f"whats {a} {name} {b}?",
                f"{a} {sym} {b}",
                f"can you work out {a} {name} {b} for me",
                f"hey quick one, {a} {name} {b}",
            ]
            out.append(case(random.choice(forms), want, f"arith-{name}"))
    for _ in range(many(40)):
        a = random.randint(2, 40)
        b = random.randint(2, 4)
        out.append(case(random.choice([
            f"what is {a} to the power of {b}",
            f"{a} to the power of {b}",
            f"raise {a} to the {b}",
        ]), str(a ** b), "arith-pow"))
    for _ in range(many(40)):
        a, b = random.randint(2, 40), random.randint(2, 12)
        out.append(case(random.choice([
            f"what is {a * b} divided by {b}",
            f"{a * b} / {b}",
            f"divide {a * b} by {b}",
        ]), str(a), "arith-div"))
    for _ in range(many(30)):
        a, b = random.randint(10, 90), random.randint(3, 9)
        out.append(case(random.choice([
            f"what is the remainder of {a} divided by {b}",
            f"{a} mod {b}",
        ]), str(a % b), "arith-mod"))
    for _ in range(many(40)):
        a, b = random.randint(2, 30), random.randint(2, 30)
        out.append(case(f"is {a} bigger than {b}", "true" if a > b else "false", "compare"))
    for _ in range(many(20)):
        n = random.randint(1, 200)
        out.append(case(random.choice([
            f"is {n} even", f"is {n} even?", f"is {n} an even number",
        ]), "true" if n % 2 == 0 else "false", "compare"))
    return out


def counting():
    out = []
    for _ in range(many(80)):
        w = random.choice(WORDS)
        ch = random.choice(sorted(set(w)))
        want = str(w.count(ch))
        forms = [
            f"how many times does the letter {ch} appear in {w}",
            f"how many {ch} characters are in {w}",
            f"count the letter {ch} in {w}",
            f"number of {ch} letters in {w}",
            f'how many times does "{ch}" appear in {w}',
        ]
        out.append(case(random.choice(forms), want, "count-chars"))
    # Typos get their own cases, with the answer computed from the word as
    # actually typed. Expecting the count for the correct spelling would be
    # grading Spoon on a question nobody asked.
    for _ in range(many(30)):
        w = typo(random.choice(WORDS))
        ch = random.choice(sorted(set(w)))
        out.append(case(
            random.choice([
                f"how many {ch} characters in {w}",
                f"count the letter {ch} in {w}",
            ]),
            str(w.count(ch)),
            "count-chars-typo",
        ))
    return out


def strings():
    out = []
    for _ in range(many(50)):
        w = random.choice(WORDS)
        out.append(case(random.choice([
            f"reverse the word {w}", f"reverse {w}", f"spell {w} backwards",
            f"what is {w} backwards",
        ]), f'"{w[::-1]}"', "reverse"))
    for _ in range(many(40)):
        w = random.choice(WORDS)
        out.append(case(random.choice([
            f"how long is the word {w}", f"how many letters in {w}",
            f"what is the length of {w}",
        ]), str(len(w)), "text-length"))
    for _ in range(many(30)):
        w = random.choice(WORDS)
        out.append(case(random.choice([
            f"uppercase {w}", f"make {w} uppercase", f"shout {w}",
        ]), f'"{w.upper()}"', "upper"))
    for _ in range(many(30)):
        w = random.choice(WORDS)
        sub = w[1:4]
        out.append(case(random.choice([
            f"does {w} contain {sub}", f"is {sub} in {w}?",
        ]), "true", "contains"))
        other = "zqx"
        out.append(case(f"does {w} contain {other}", "false", "contains"))
    for _ in range(many(25)):
        w = random.choice(WORDS)
        out.append(case(random.choice([
            f"does {w} start with {w[0]}", f"does {w} begin with {w[:2]}",
        ]), "true", "starts-with"))
    return out


def collections():
    out = []
    for _ in range(many(50)):
        xs = [random.randint(1, 60) for _ in range(random.randint(3, 6))]
        shown = ", ".join(str(x) for x in xs)
        out.append(case(random.choice([
            f"what is the sum of {shown}",
            f"add up {shown}",
            f"total of {shown}",
        ]), str(sum(xs)), "sum"))
        out.append(case(random.choice([
            f"what is the biggest of {shown}",
            f"whats the largest number in {shown}",
            f"max of {shown}",
        ]), str(max(xs)), "max"))
        out.append(case(random.choice([
            f"smallest of {shown}", f"what is the minimum of {shown}",
        ]), str(min(xs)), "min"))
        out.append(case(random.choice([
            f"how many numbers are in {shown}", f"count of {shown}",
        ]), str(len(xs)), "count"))
    return out


def lists():
    """Ranges, filters and the shapes that come back as lists."""
    out = []
    for _ in range(many(40)):
        hi = random.randint(3, 9)
        want = "list<" + ", ".join(str(i) for i in range(1, hi + 1)) + ">"
        out.append(case(random.choice([
            f"the numbers from 1 to {hi}",
            f"list the numbers 1 through {hi}",
            f"give me 1 to {hi}",
        ]), want, "range"))
    for _ in range(many(40)):
        hi = random.choice([6, 8, 10, 12])
        evens = [i for i in range(1, hi + 1) if i % 2 == 0]
        want = "list<" + ", ".join(str(i) for i in evens) + ">"
        out.append(case(random.choice([
            f"the even numbers from 1 to {hi}",
            f"which numbers between 1 and {hi} are even",
        ]), want, "filter-even"))
    for _ in range(many(40)):
        xs = [random.randint(1, 40) for _ in range(random.randint(3, 5))]
        shown = ", ".join(str(x) for x in xs)
        want = "list<" + ", ".join(str(x) for x in sorted(xs)) + ">"
        out.append(case(random.choice([
            f"sort {shown}",
            f"put {shown} in order",
            f"sort these numbers: {shown}",
        ]), want, "sort"))
        rev = "list<" + ", ".join(str(x) for x in reversed(xs)) + ">"
        out.append(case(f"reverse the list {shown}", rev, "reverse-list"))
    for _ in range(many(30)):
        xs = [random.randint(1, 9) for _ in range(6)]
        shown = ", ".join(str(x) for x in xs)
        want = "list<" + ", ".join(str(x) for x in sorted(set(xs), key=xs.index)) + ">"
        out.append(case(random.choice([
            f"remove the duplicates from {shown}",
            f"what are the unique values in {shown}",
        ]), want, "unique"))
    for _ in range(many(30)):
        xs = [random.randint(1, 40) for _ in range(4)]
        shown = ", ".join(str(x) for x in xs)
        out.append(case(random.choice([
            f"what is the first of {shown}",
            f"first item in {shown}",
        ]), str(xs[0]), "first"))
        out.append(case(f"what is the product of {shown}", str(
            xs[0] * xs[1] * xs[2] * xs[3]), "product"))
    return out


def more_strings():
    out = []
    for _ in range(many(30)):
        w = random.choice(WORDS)
        out.append(case(random.choice([
            f"lowercase {w.upper()}", f"make {w.upper()} lowercase",
        ]), f'"{w}"', "lower"))
    for _ in range(many(30)):
        w = random.choice(WORDS)
        a, b = w[0], "z"
        out.append(case(random.choice([
            f"replace every {a} in {w} with {b}",
            f"swap every {a} in {w} for {b}",
            f'replace "{a}" with "{b}" in {w}',
        ]), f'"{w.replace(a, b)}"', "replace"))
    for _ in range(many(30)):
        w = random.choice(WORDS)
        n = random.randint(2, min(5, len(w) - 1))
        out.append(case(random.choice([
            f"the first {n} letters of {w}",
            f"first {n} characters of {w}",
        ]), f'"{w[:n]}"', "substring"))
    for _ in range(many(30)):
        parts = random.sample(WORDS, 3)
        joined = ",".join(parts)
        want = "list<" + ", ".join(f'"{p}"' for p in parts) + ">"
        out.append(case(random.choice([
            f"split {joined} on commas",
            f"break {joined} apart at the commas",
        ]), want, "split"))
    for _ in range(many(25)):
        w = random.choice(WORDS)
        out.append(case(f"does {w} end with {w[-2:]}", "true", "ends-with"))
        out.append(case(f"does {w} end with qz", "false", "ends-with"))
    for _ in range(many(25)):
        w = random.choice(WORDS)
        out.append(case(random.choice([
            f'trim the spaces off "  {w}  "',
            f'strip the whitespace from "  {w}  "',
        ]), f'"{w}"', "trim"))
    return out


def logic():
    out = []
    for _ in range(many(40)):
        a, b = random.choice([True, False]), random.choice([True, False])
        sa, sb = str(a).lower(), str(b).lower()
        out.append(case(f"is {sa} and {sb}", str(a and b).lower(), "and"))
        out.append(case(f"is {sa} or {sb}", str(a or b).lower(), "or"))
    for _ in range(many(20)):
        n = random.randint(-40, 40)
        out.append(case(f"is {n} negative", str(n < 0).lower(), "sign"))
        out.append(case(f"is {n} zero", str(n == 0).lower(), "sign"))
    return out


NAMES = ["alice", "bob", "carol", "dave", "erin", "frank", "grace", "heidi",
         "ivan", "judy", "mallory", "olivia", "peggy", "trent", "victor", "walter"]


def setup(say, category):
    return {"say": say, "expect": None, "category": category, "setup": True}


def inference(seed):
    """The meta-vocabulary, tested the only way it means anything: say a
    thing, then ask something that was never said.

    Yes-or-no questions on purpose. A question with a hole comes back as the
    list of facts that satisfy it, and grading that shape would be grading the
    renderer rather than the inference.
    """
    # Disjoint casts, so the test half asks about people the training half
    # never mentioned. Reusing the names would score memory rather than
    # inference, which is the one thing this suite exists to measure.
    out = []
    rng = random.Random(seed)
    pool = [f"{n}-{seed}" for n in NAMES]
    rng.shuffle(pool)

    def take(n):
        if len(pool) < n:
            pool.extend(f"{x}-{seed}-{len(pool)}" for x in NAMES)
        return [pool.pop() for _ in range(n)]

    for rel, phrase in [("friends-with", "friends with"),
                        ("married-to", "married to"),
                        ("sibling-of", "a sibling of")]:
        out.append(setup(f"{rel} is symmetric", "symmetric"))
        for _ in range(many(12)):
            a, b = take(2)
            out.append(setup(f"{a} is {phrase} {b}", "symmetric"))
            out.append(case(f"is {b} {phrase} {a}", "true", "symmetric"))

    for rel, phrase in [("part-of", "part of"), ("ancestor-of", "an ancestor of")]:
        out.append(setup(f"{rel} is transitive", "transitive"))
        for _ in range(many(10)):
            a, b, c = take(3)
            out.append(setup(f"{a} is {phrase} {b}", "transitive"))
            out.append(setup(f"{b} is {phrase} {c}", "transitive"))
            out.append(case(f"is {a} {phrase} {c}", "true", "transitive"))

    out.append(setup("parent-of is the inverse of child-of", "inverse"))
    for _ in range(many(12)):
        a, b = take(2)
        out.append(setup(f"{a} is the parent of {b}", "inverse"))
        out.append(case(f"is {b} the child of {a}", "true", "inverse"))

    for _ in range(many(10)):
        a = take(1)[0]
        out.append(setup(f"a {a}-fish is a subtype of fish", "subtype"))
        out.append(setup("a fish is a subtype of animal", "subtype"))
        out.append(case(f"is a {a}-fish an animal", "true", "subtype"))

    return out


FILLER_OPENERS = [
    "ok so", "alright", "hey", "quick one", "sorry", "um", "ok wait",
    "one more", "lol ok", "hmm", "so like", "another one",
]

FILLER_TAILS = [
    "", "", "", " please", " thanks", " lol", " ?", " if you can", " real quick",
]


def messy():
    """The same questions, typed the way people type them.

    Openers, trailing filler, no capitals, no punctuation. A corpus where every
    case is well formed trains recognition of a template, and a template is not
    what arrives.
    """
    out = []
    for _ in range(many(120)):
        a, b = random.randint(2, 200), random.randint(2, 99)
        op, want = random.choice([
            ("plus", a + b), ("minus", a - b), ("times", a * b),
        ])
        say = (f"{random.choice(FILLER_OPENERS)} whats {a} {op} {b}"
               f"{random.choice(FILLER_TAILS)}")
        out.append(case(say, str(want), "messy-arith"))
    for _ in range(many(60)):
        w = random.choice(WORDS)
        say = (f"{random.choice(FILLER_OPENERS)} reverse {w}"
               f"{random.choice(FILLER_TAILS)}")
        out.append(case(say, f'"{w[::-1]}"', "messy-reverse"))
    for _ in range(many(60)):
        w = random.choice(WORDS)
        ch = random.choice(sorted(set(w)))
        say = (f"{random.choice(FILLER_OPENERS)} how many {ch} characters in {w}"
               f"{random.choice(FILLER_TAILS)}")
        out.append(case(say, str(w.count(ch)), "messy-count"))
    return out


def corrections():
    """A speaker repairing what they just said.

    The repair is the whole test: the answer is about the second thing, and a
    turn that answers the first has understood the words and missed the point.
    """
    out = []
    for _ in range(many(40)):
        a, b = random.sample(WORDS, 2)
        out.append(case(random.choice([
            f"reverse {a}, no wait, reverse {b}",
            f"reverse {a}. actually no, {b}",
            f"reverse {a} sorry i meant {b}",
        ]), f'"{b[::-1]}"', "correction"))
    for _ in range(many(40)):
        x, y = random.randint(2, 90), random.randint(2, 90)
        z = random.randint(2, 90)
        out.append(case(random.choice([
            f"what is {x} plus {y}, no wait, {x} plus {z}",
            f"whats {x} plus {y}. scratch that, {x} plus {z}",
        ]), str(x + z), "correction"))
    return out


def build(name, note, cases):
    path = OUT / f"{name}.json"
    path.write_text(json.dumps(
        {"name": name, "note": note, "cases": cases}, indent=1) + "\n")
    print(f"{len(cases):>5}  {path}")


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    full = (arithmetic() + counting() + strings() + collections()
            + lists() + more_strings() + logic() + messy() + corrections())
    # Deduplicate by utterance. Generators repeat themselves, and the same
    # sentence landing in both halves would put a test case in the training
    # set, which is the one mistake that makes every later number a lie.
    seen, unique = set(), []
    for c in full:
        if c["say"] not in seen:
            seen.add(c["say"])
            unique.append(c)
    full = unique
    random.shuffle(full)
    build("graded", "Everything checkable, asked several ways each. The night run.", full)

    # A stratified sample for iteration: same category mix, small enough to run
    # between edits. Bisecting a regression needs a suite you will actually run.
    by_cat = {}
    for c in full:
        by_cat.setdefault(c["category"], []).append(c)
    core = []
    for cat in sorted(by_cat):
        core.extend(by_cat[cat][:6])
    build("graded_core", "A stratified sample of graded, small enough to run between edits.", core)

    # Never shuffled. Each question depends on the turns before it, which is
    # the whole point: the answer was never said, only implied.
    # Split before anything learns, so a score on the test half is a score on
    # utterances Spoon has never been shown. Running a corpus with teaching on
    # is training; scoring on that same corpus measures memorization.
    #
    # Split within each category so both halves cover the same ground: a test
    # set that happened to lose every arithmetic case would look like progress.
    train, test = [], []
    for cat in sorted(by_cat):
        items = by_cat[cat][:]
        random.Random(11).shuffle(items)
        cut = int(len(items) * 0.7)
        train.extend(items[:cut])
        test.extend(items[cut:])
    random.Random(12).shuffle(train)
    random.Random(13).shuffle(test)
    build("graded_train", "The half Spoon is allowed to learn from.", train)
    build("graded_test", "Held out. Never run with teaching on before scoring.", test)

    build("graded_facts_train",
          "Say a thing, then ask something that was never said. Order matters.",
          inference("a"))
    build("graded_facts_test",
          "The same shapes about a cast the training half never mentioned.",
          inference("b"))


if __name__ == "__main__":
    main()
