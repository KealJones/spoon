# Spoon - Agent Instructions

## No Placeholders, Stubs, or Skeletons

NEVER add placeholder, skeleton, fake, "not yet implemented", TODO-gated, or otherwise bogus implementations. EVER. Every implementation must be fully functional the first time it is written. If a feature requires work you cannot complete in the current scope, do NOT leave a stub - ask the user directly whether to implement it now or skip it entirely. The user cannot approve skeleton implementations through a proposed plan - they might have missed that detail. You must ask them explicitly and get direct confirmation before leaving anything unimplemented.

This includes:
- Functions that log "not yet implemented" and return early
- Match arms that skip functionality with a diagnostic string
- Empty trait implementations
- Feature flags that gate non-existent code
- Comments like "TODO: implement this later"

If you write it, it works. Period.
