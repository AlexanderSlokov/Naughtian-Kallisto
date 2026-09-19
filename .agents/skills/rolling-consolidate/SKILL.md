---
name: rolling-consolidate
description: Consolidate a long source document (chat transcript, meeting notes, issue thread, research dump, RFC comments) into one structured deliverable such as an ADR, spec, design doc, or report. Use when the source is too long to read in a single pass, when later statements in it override earlier ones, or when the user asks to distill a session or thread into a document.
---

# Rolling consolidate

Turn one long, messy, chronological source into one structured deliverable, by reading the source in order, in chunks, and writing findings to an external ledger file instead of holding them in context.

## When this applies

Use it when all three are true:

- The source is chronological and long enough that reading it whole would crowd out the work.
- Positions inside it evolve. Later passages revise or contradict earlier ones.
- The output is a single document with a fixed shape (a template, or sibling documents to match).

Do not use it for short sources, for sources where nothing supersedes anything, or when the user wants a summary rather than a consolidated document. A summary compresses. This produces a document that stands on its own and can be acted on.

## Method

### 0. Read the target shape first

Before opening the source, read the template and one or two sibling documents already in the repo. Note the frontmatter fields, section order, heading language, and whether the prose is English or another language. The deliverable has to sit next to its siblings without looking foreign.

Also check how siblings are numbered and whether the user asked for a specific identifier.

### 1. Size the source

Get the line count. Pick a chunk size that leaves room to think, usually 100-150 lines. Announce the plan to the user in one line before starting.

### 2. Read in order, one chunk at a time

Read chunk N. Do not skim ahead. Do not read the whole file "just to get oriented" — that is the failure this method exists to prevent.

### 3. After each chunk, append to the ledger

Write findings to a scratch ledger file immediately, then move to the next chunk. The ledger lives outside the deliverable, in a scratch directory.

One entry per idea, numbered continuously across chunks:

```
- I17 [speaker] TAG: the claim, in enough detail to rebuild the argument later.
  Consequence or constraint it implies.
  NOTE: relationship to other entries.
```

Rules for entries:

- **Number continuously.** I1, I2, I3 across the whole pass. The numbers become the audit trail.
- **Attribute the speaker.** In a two-party discussion, a constraint the user stated and a suggestion the assistant offered carry different weight at assembly time. Keep them distinguishable.
- **Tag the kind.** Decision, constraint, non-goal, threat, open question, naming, sequencing, rejected option. Tags drive which section of the deliverable the entry lands in.
- **Preserve specifics.** Numbers, names, error codes, file paths, library names, exact phrasings the user coined. Specifics are the whole value; a paraphrase loses them and cannot be recovered later without re-reading.
- **Copy reasoning, not just conclusions.** An entry that records "chose X" without "because Y" produces a deliverable nobody can revisit.

### 4. Apply last-come-wins, and record the loss

When a later entry conflicts with an earlier one, the later entry wins by default, because the discussion moved.

Two qualifications:

- **Later wins on merit, not on position alone.** If the later statement is a passing aside and the earlier one was reasoned, flag both and resolve it at assembly, or ask the user. Restating a point is not the same as revising it.
- **Never delete the superseded entry.** Mark it. Write `SUPERSEDED BY I54` on the loser and `invalidates I17` on the winner. The deliverable will need a table of what changed and why, and that table cannot be reconstructed from a ledger that quietly dropped things.

Partial supersession is common. A later entry often overrides one clause of an earlier one and leaves the rest standing. Record which clause.

### 5. Assemble only after the full pass

Do not start writing the deliverable until the source is exhausted. Early assembly bakes in positions the source later revised.

At assembly:

- Map ledger tags onto template sections. Decisions and constraints become the body; non-goals become an explicit limits section; rejected options become the options comparison.
- Every claim in the deliverable traces back to a ledger id. If it does not, it came from somewhere other than the source, so either cut it or mark it as your own inference.
- Write nothing the source does not support. Gaps are findings, not invitations to fill in.
- Build the document in appends, section by section, rather than one large write.

### 6. Hand back

Report to the user:

- Where the deliverable landed, and its size.
- The supersession table: what the source revised over its own course.
- What you left unresolved and why, including anything that needed their decision.
- Where the ledger is, so they can trace any claim back to its passage.

## Failure modes

**Swallowing the source whole.** Context fills, detail blurs, and the output drifts toward plausible-sounding invention. The chunk loop exists to prevent exactly this, so do not shortcut it because the file "looks manageable."

**Summarizing per chunk instead of harvesting.** A chunk summary throws away the specifics that make the deliverable useful. Harvest claims; do not compress passages.

**Silent supersession.** Dropping the earlier position leaves the reader unable to see that a decision was reconsidered, which is often the most valuable thing the source contains.

**Template drift.** Inventing sections because the material seemed to want them. Match the siblings; put anything extra in a clearly additional section.

**Flattening attribution.** Losing track of who asserted what turns a negotiated decision into an undifferentiated wall of assertions.

## Worked example

Source: a 745-line exported chat transcript where a project's scope was renegotiated end to end. Target: an ADR matching ten siblings.

Read the ADR template and two recent siblings. Read the transcript in six chunks of roughly 120 lines. Append after each chunk, reaching 94 numbered entries. Six conflicts surfaced, each recorded on both sides. Assemble the ADR in four appends: context and decision, then detailed decisions, then consequences and options, then the impact table on prior documents. Hand back the deliverable plus the supersession table plus two questions the source left open.
