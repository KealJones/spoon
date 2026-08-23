# spoon-credit

Evidence-bounded credit assignment for failed SPOON episodes.

## Owns

- Contract-violation attribution from recorded execution checks.
- Statistical suspect ranking from indexed episode aggregates.
- Budgeted, version-pinned counterfactual replay and provenance.

## How it works with the system

`spoon-engine` provides the failed episode, exact procedure snapshots, and
replay budget. This crate returns ranked suspects and confidence/limitation
metadata; `spoon-adapt` decides whether any correction is authorized. Suspicion
alone never becomes a graph mutation.
