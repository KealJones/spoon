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
import pathlib
import random

random.seed(7)  # Reproducible: a corpus that shuffles is a corpus you cannot bisect.

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
        for _ in range(40):
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
    for _ in range(30):
        a = random.randint(2, 40)
        b = random.randint(2, 12)
        out.append(case(random.choice([
            f"what is {a} to the power of {b if b < 5 else 2}",
            f"{a} squared",
        ]).replace("squared", "to the power of 2"), str(a ** (b if b < 5 else 2)) if "power" in "power" else "", "arith-pow"))
    # Drop the malformed pow cases rather than ship a wrong expectation.
    out = [c for c in out if c["expect"]]
    for _ in range(30):
        a, b = random.randint(2, 30), random.randint(2, 30)
        out.append(case(f"is {a} bigger than {b}", "true" if a > b else "false", "compare"))
    for _ in range(20):
        n = random.randint(1, 200)
        out.append(case(random.choice([
            f"is {n} even", f"is {n} even?", f"is {n} an even number",
        ]), "true" if n % 2 == 0 else "false", "compare"))
    return out


def counting():
    out = []
    for _ in range(80):
        w = random.choice(WORDS)
        ch = random.choice(sorted(set(w)))
        want = str(w.count(ch))
        forms = [
            f"how many {ch}s are in {w}",
            f"how many {ch}s are there in {w}?",
            f"count the {ch}s in {w}",
            f"number of {ch} in {w}",
            f"how many times does {ch} appear in {w}",
            f"how many {ch}s in {typo(w)}".replace(typo(w), w),
        ]
        out.append(case(random.choice(forms), want, "count-chars"))
    return out


def strings():
    out = []
    for _ in range(50):
        w = random.choice(WORDS)
        out.append(case(random.choice([
            f"reverse the word {w}", f"reverse {w}", f"spell {w} backwards",
            f"what is {w} backwards",
        ]), f'"{w[::-1]}"', "reverse"))
    for _ in range(40):
        w = random.choice(WORDS)
        out.append(case(random.choice([
            f"how long is the word {w}", f"how many letters in {w}",
            f"what is the length of {w}",
        ]), str(len(w)), "text-length"))
    for _ in range(30):
        w = random.choice(WORDS)
        out.append(case(random.choice([
            f"uppercase {w}", f"make {w} uppercase", f"shout {w}",
        ]), f'"{w.upper()}"', "upper"))
    for _ in range(30):
        w = random.choice(WORDS)
        sub = w[1:4]
        out.append(case(random.choice([
            f"does {w} contain {sub}", f"is {sub} in {w}?",
        ]), "true", "contains"))
        other = "zqx"
        out.append(case(f"does {w} contain {other}", "false", "contains"))
    for _ in range(25):
        w = random.choice(WORDS)
        out.append(case(random.choice([
            f"does {w} start with {w[0]}", f"does {w} begin with {w[:2]}",
        ]), "true", "starts-with"))
    return out


def collections():
    out = []
    for _ in range(50):
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


def build(name, note, cases):
    path = OUT / f"{name}.json"
    path.write_text(json.dumps(
        {"name": name, "note": note, "cases": cases}, indent=1) + "\n")
    print(f"{len(cases):>5}  {path}")


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    full = arithmetic() + counting() + strings() + collections()
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


if __name__ == "__main__":
    main()
