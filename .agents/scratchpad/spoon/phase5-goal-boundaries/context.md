# Context

Phase 5 requires task and standing goals to be externally supplied, derived goals to remain
in service of those goals, and every learning goal to be traceable to a standing goal. The
existing goal store persisted a generic parent ID but allowed callers to create root learning
goals and retained no durable record of the curiosity gap or derivation reason.

This track is intentionally bounded to the engine goal store and public engine facade. It does
not add a structural self-modification mechanism or change server/SDK protocol surfaces.
