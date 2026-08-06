# Addendum — Signal workstream (supersedes prior schema where it conflicts)

## Schema additions (Workstream A)
Add to /fosfora/v1/:
  /phrase/bar         i  (bar within phrase)   continuous
  /phrase/len         i  (inferred 8|16|32)    on change
  /phrase/beats_left  i                        continuous
  /predict/drop       f  confidence 0..1       continuous

## Phrase-grid tracker (new component in A)
Downbeat counting → phrase-length inference (8/16/32 bars; 4/4 assumption with
confidence, degrade gracefully off-grid). Purpose: LEAD TIME for the operator —
detection alone is post-hoc. `predict/drop` fuses build-ramp slope with
distance-to-phrase-boundary. This is the differentiator over beat→OSC bridges
and ShowKontrol's hardware-locked pre-analysis.

## Positioning guardrails (product constraints, not docs-only)
- Every stateful signal carries a confidence value; consumers can ignore it.
- Signal never triggers anything itself — it informs the operator's rig.
- Naming/docs frame it as telemetry for humans; never "automation of the
  performance."

## Benchmark harness additions (Workstream C)
For predict/drop: measure lead-time distribution (beats before annotated drop
that confidence crosses 0.5 / 0.8) and false-alarm rate. Lead time is the
product claim — it gets measured, not asserted.
