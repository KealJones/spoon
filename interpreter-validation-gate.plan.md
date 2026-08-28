# Plan

1. Add a regression test proving a locally resolvable procedure requests
   interpreter validation when `interpreter_allowed` is true.
2. Change the local fast-path gate so it executes immediately only when the
   interpreter is disabled.
3. Run cycle tests, rebuild the server, and manually verify a known procedure
   now reports `Interpreter: attempted`.

The interpreter remains a validator/candidate selector, not an authority
boundary. Deterministic admin and capability authorization paths stay outside
this change.
