//! Signal — the analysis engine as a product: a headless mode that broadcasts
//! Fosfora's musical understanding (beats, bars, sections, phrase position, drop
//! prediction, stem-proxy energies, the raw feature bus) over versioned OSC
//! addresses for TouchDesigner / Resolume / QLC+ / grandMA-class consumers.
//!
//! The contract lives in `docs/SIGNAL.md` and `schema.rs`. Positioning guardrails
//! (program addendum): every stateful signal carries a confidence value, and
//! Signal only ever *informs* the operator's rig — it triggers nothing itself.

pub mod clock;
pub mod phrase;
pub mod schema;
pub mod section;
pub mod sink;
