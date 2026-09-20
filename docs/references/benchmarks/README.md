# Performance reports (benchmarks)

An archive of benchmark results across Kallisto's development, kept in date order. Most of it is
**history rather than description**: the reports below predate ADR-0015, and the mechanisms they
measure — SipHash and a sharded cuckoo table, a B-tree path index, write-behind offloading I/O to
RocksDB — were all deleted along with the storage engine. A report saying `Admin Port: 8202` or
`BATCH mode` is measuring a system that no longer exists.

Kept anyway, because a decision that cites a measurement should leave the measurement where it can
be checked. ADR-0016 rests on numbers from this archive, and `verification-status.md` distinguishes
what is proven from what is merely believed.

**For what the current read path measures, see
[serving-kv-secrets.md](../../explanation/why-use-naughtian-kallisto/serving-kv-secrets.md).** That
page is the canonical one, and the method behind the two cost figures is in
[plans/duck/plan.md](../plans/duck/plan.md).

Reproduce locally with `make bench-laptop` or `make bench-duck`; `cargo bench` runs the in-process
Criterion suite for the barrier.
