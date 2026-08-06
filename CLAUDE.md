# Fosfora — project instructions

## Writing CHANGELOG entries

`CHANGELOG.md` is the **release notes**. CI extracts each `## vX.Y.Z` section verbatim
into the GitHub release body, so every entry is read by users, not by us.

**One to three sentences per entry.** Hard ceiling ~600 characters. If an entry needs
more, it is carrying material that belongs somewhere else.

Each entry answers only:
1. What can a user do now that they couldn't before, or what stopped being broken?
2. What might break for them, and what do they do about it?

For a *fixed bug*, the symptom the user actually saw is the most valuable sentence —
lead with it ("kick-bound strobes fired on hi-hat bleed", "horizontal surfaces swallowed
particles instead of bouncing them"). One clause of cause is plenty. Keep a measured
before/after when it is short and concrete ("spurious zeros 164 → 0") — that is evidence,
not padding.

**Do not put in the changelog:**
- Implementation walkthroughs — algorithms, data structures, shader internals, buffer
  layouts, dispatch counts, workgroup sizes, file and function names.
- Design rationale — why an approach was chosen, what was ruled out, what a reviewer
  might otherwise object to. This is commit-body and design-doc material.
- Internal issue numbers (`#1796`, `board #1857`). A reader outside the repo cannot open
  them. Reference them in the commit message instead.
- Verification narratives. State the result in a clause if it is meaningful to a user
  ("verified live at ~214K particles above 100 FPS"), not the harness, the env vars, or
  the sequence of rounds it took.
- Development history of something shipping in this same release. If an effect landed,
  was found to look wrong, and was fixed before release, users only ever see the final
  thing — write one entry for the effect, not one per iteration.

ABI and layout changes are the exception: state the byte sizes plainly, because they tell
a user whether their own shaders need recompiling.

Keep the `## vX.Y.Z — date` header format and re-header `## Unreleased` at each release
(CI reads the version from `crates/fosfora-app/Cargo.toml`), or notes bleed between
sections.

**Calibration:** the v1.8.0 section is the target shape (~250 chars per entry). The
pre-rewrite v1.9.0–v1.12.0 sections are the anti-pattern — entries of 1,400+ characters
averaging six times the prose per unit of work.

## Releasing

Bump the version in `crates/fosfora-app/Cargo.toml`, update `CHANGELOG.md`, push to main.
CI auto-tags and releases. Never `git tag` by hand.
