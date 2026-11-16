---
title: Run the benchmarks
description: Measure Kallisto's read path, its CPU and RAM cost, and the in-memory barrier on your own machine.
sidebar:
  order: 8
---

Kallisto makes performance claims, so it ships the means to check them. Every
harness here measures the **read path**, because there is no write path left to
measure.

## Before you start

- A source checkout that builds. See [build from
  source](/kallisto/how-to/build-from-source/).
- `wrk2`, built from `github.com/giltene/wrk2`, for the HTTP benchmarks.
- `python3`, for `make bench-profile`.

## Read-path latency at a fixed rate

```bash
make bench-laptop
```

Seeds a sealed file, starts the server with 4 workers, and drives it with
`wrk2` at 30,000 requests per second over 100 connections for 10 seconds. On
the reference laptop the expected result is a p50 around 1.3 ms.

For your own parameters, call the script directly. Every argument is optional:

```bash
make build-server build-ctl
bash benchmarks/server/run_duck_bench.sh [workers] [connections] [duration] [rate]

# what bench-laptop runs
bash benchmarks/server/run_duck_bench.sh 4 100 10s 30000
```

`make bench-duck` runs the script with its defaults: half the physical cores as
workers, 100 connections, 10 seconds, and an offered rate of 200,000 requests
per second, which saturates most machines.

`wrk2` holds the request rate constant while concurrency varies. That
avoids coordinated omission, where a stalled server looks faster because the
load generator stopped sending while it waited. For a process on the request
path, tail latency under a sustained rate is the number that matters.

## CPU and RAM from idle to saturated

```bash
make bench-profile
```

Takes about 18 minutes. It measures idle, sweeps the offered rate from 1,000 to
60,000 requests per second, then saturates the server. It repeats three times,
takes medians, and draws the load-against-resource chart. Results go under
`target/bench/`, so a run on your machine never overwrites the published
figures.

## The in-memory barrier

```bash
cargo bench
```

Runs the in-process Criterion benchmark in `benchmarks/barrier/`. It needs no
extra tooling. It measures what sealing each secret in RAM between requests
costs, which on the reference machine is 350 to 430 ns per request.

## Getting numbers you can compare

- **Record the hardware.** The server pins one worker per core and shares
  nothing between them, so results depend on core count, SMT layout and pinning.
- **Interleave comparisons.** Measuring two builds one after the other on a
  laptop once made the slower build look faster, because the machine was warming
  up. Run both binaries alternately inside one loop.
- **Separate server and load generator** if you can. The published figures put
  both on one machine, pinned to different logical CPUs, and should be read as
  an order of magnitude.

The published figures are in [performance and
footprint](/kallisto/references/performance/).

:::note[Older reports]
The archive under `docs/references/benchmarks/` in the Kallisto repository
mostly predates 2.0.0. Its PUT figures measure a write path that no longer
exists, and its C++ and DragonflyDB comparisons describe earlier
architectures. They are kept as history.
:::
