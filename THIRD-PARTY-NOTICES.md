# Third-party notices

Naughtian Kallisto is licensed under AGPL-3.0-or-later (see `LICENSE`).

Parts of this repository are derived from third-party projects under their own
licenses. This file records what was taken, from where, and what was changed, as
required by those licenses.

## tikv/tikv

Upstream: https://github.com/tikv/tikv
License: Apache License 2.0 (full text in `LICENSE-APACHE-2.0`)
Copyright: The TiKV Authors
Revision referenced: `1ef0a1961c2f1647736730dfe72eefd8251ab778` (2026-08-25)

Apache-2.0 permits inclusion in an AGPL-3.0 work; the combined work is governed
by the AGPL. The reverse does not hold — no Kallisto code may be taken upstream
under these terms.

Derived files, all modified for Kallisto:

- `rustfmt.toml` — adopted verbatim from upstream.
- `clippy.toml` — adopted from upstream, then edited: entries whose target crate
  is absent from the Kallisto dependency graph were dropped, and the rationale
  strings for the Tokio runtime hooks were rewritten to describe Kallisto's own
  worker construction path.
- `deny.toml` — adopted from upstream, then edited: the `wrappers` allowances for
  TiKV's cloud-storage dependencies were removed, the advisory ignore list was
  replaced, and the licensing rationale was rewritten for AGPL.

Note that lint selections, dependency policies, and other configuration decisions
are not themselves copyrightable; the attribution above covers the explanatory
prose carried over with them.

## Naming

"TiKV" is a trademark of its respective owners. It is referenced here and in the
documentation only to describe provenance and prior art, not to imply any
endorsement of or affiliation with Naughtian Kallisto.
