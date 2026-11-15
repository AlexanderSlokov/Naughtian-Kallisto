# Kallisto engineering documentation

These pages are for whoever changes the code. They travel with it, get reviewed in the pull request
that changes the behaviour they describe, and are cited directly from source comments — 38 Rust
files reference an ADR by number.

**If you want to run Kallisto rather than change it, start at
[docs.naughtian.org/kallisto](https://docs.naughtian.org/kallisto/).** That site is the front door,
and it carries the quickstart and the operational guides.

## Pages that come from the site

A page with a `title` in its front matter is a copy of a page on docs.naughtian.org. These are
`index.mdx` and the pages under `tutorials/`, `how-to/`, and the top level of `explanation/` and
`references/`. Helvilette Mannaz delivers each change to them as a pull request from the
`mannaz/sync` branch.

To change one of these pages, change it in the `naughtian-website` repository, under
`src/content/docs/kallisto/`. An edit made here stops the next sync until the site has the same
edit. Links between these pages use site paths, such as `/kallisto/references/configuration/`, so
they work on the site only.

The other pages stay in this repository, and the site does not show them.

## Where things are

[explanation/](./explanation/) — what the system is and why it has this shape.
[architecture.md](./explanation/architecture.md) is the overview; the deep dive, including the two
architectures that were abandoned, is in
[how-to-create-naughtian-kallisto/](./explanation/how-to-create-naughtian-kallisto/).

[references/](./references/) — the record of decisions and evidence.
[ADRs/](./references/ADRs/) holds the decision log, and ADR-0015 and ADR-0016 are the two that
define the system as it now stands. [verification-status.md](./references/verification-status.md)
records what is actually proven versus merely believed, and
[benchmarks/](./references/benchmarks/) holds the measurements.
[roadmap.md](./references/roadmap.md) is what is planned.

[operations/](./operations/) — building it.
[development/toolchain-setup.md](./operations/development/toolchain-setup.md) is the one to read
before your first build.

## Reading the ADRs

Two of them carry the current design, and the rest are history worth keeping:

- **ADR-0015** redefined the problem from "a high-performance secrets server" to "a resolver for one
  machine's apps", and deleted roughly nine thousand lines.
- **ADR-0016** amended two of its details after the read path turned out to be hot: it kept
  thread-per-core, and welded the output port shut while keeping a real port at the source.

An ADR that has been superseded says so in its own front matter rather than being deleted, because
"we decided this and then changed our minds" and "we never considered it" are different facts.
