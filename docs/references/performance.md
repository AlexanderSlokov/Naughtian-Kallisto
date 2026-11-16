---
title: Performance and footprint
description: Published CPU, memory, throughput and latency figures for the 2.0.0 read path, and how they were measured.
sidebar:
  order: 6
---

## Resource profile

Measured by the project on a laptop:

- HP Pavilion Gaming 15-ec0xxx, AMD Ryzen 5 3550H (4 cores, 8 threads), 13 GiB
  RAM visible to the OS, Ubuntu 24.04, on mains power.
- Default configuration: 2 workers, access log on and written to a file, a
  sealed file of 64 secrets.
- Rate limiter lifted, so the serving path is measured.
- Server on logical CPUs 0-3, load generator on 4-7 of the same machine.
- Every figure is the median of 3 runs.

| State | Requests per second | RAM (RSS) | CPU, % of one logical CPU | p50 / p99 |
|---|---|---|---|---|
| Idle | 0 | 8.4 MiB | 1.3% | - |
| Stress | 39,247 | 11.8 MiB | 132% | 1.35 ms / 3.22 ms |
| Saturated | 63,648 | 16.7 MiB | 216% | 4.04 ms / 6.04 ms |

Stress is 40,000 requests per second offered, the most the default
configuration will serve: 20,000 per worker, 2 workers. Saturated is `wrk` with
256 connections sending as fast as the server answers.

| Offered rate | Served | RAM (RSS) | CPU | CPU per request | p50 | p99 |
|---|---|---|---|---|---|---|
| 1,000 | 982 | 11.6 MiB | 14% | 143 µs | 1.02 ms | 2.62 ms |
| 5,000 | 4,981 | 12.2 MiB | 47% | 95 µs | 1.07 ms | 2.34 ms |
| 10,000 | 9,960 | 11.9 MiB | 84% | 85 µs | 1.44 ms | 3.02 ms |
| 20,000 | 19,623 | 11.5 MiB | 113% | 58 µs | 1.67 ms | 3.82 ms |
| 30,000 | 29,434 | 11.8 MiB | 118% | 40 µs | 1.32 ms | 4.13 ms |
| 40,000 | 39,247 | 11.8 MiB | 132% | 34 µs | 1.35 ms | 3.22 ms |
| 50,000 | 49,793 | 11.9 MiB | 159% | 32 µs | 1.27 ms | 4.48 ms |
| 60,000 | 59,752 | 12.0 MiB | 185% | 31 µs | 1.42 ms | 6.61 ms |

Every request in every run answered 200.

What the numbers show:

- **RAM stays flat under load**, near 12 MiB from 1,000 to 60,000 requests per
  second. The step to 16.7 MiB at saturation comes with 256 open connections
  instead of 100.
- **The ceiling is the server's.** At saturation both workers are fully busy
  while the load generator used about half its CPUs. The two workers were
  pinned to the two hardware threads of one physical core, so the default
  configuration served about 63,000 requests per second from a single core.
- **CPU per request falls as load rises**, from 143 µs to 31 µs. The likely
  cause is that one wake-up of a worker and the log writer serves many requests
  under load. This has not been measured separately.
- **Idle is near zero.** The access log writer checks for work at most every
  millisecond, which avoids putting a mutex on the read path.

## More workers

With 4 workers on an 8-core laptop, the project measured about 99,000 requests
per second at saturation, and p50 1.3 ms / p99 3.5 ms at a fixed 30,000 requests
per second.

## Cost of the protections

| Protection | Cost |
|---|---|
| In-memory barrier (each secret sealed in RAM between requests) | 350-430 ns per request |
| Access log | about 6% of saturation throughput |

Both were measured by interleaving two builds in one loop, after measuring them
one after the other gave a result explained by the laptop warming up.

## Binary size

| Binary | Size |
|---|---|
| `kallisto-server` | 7.5 MiB, 5.7 MiB stripped |
| `kallisto-ctl` | 2.4 MiB |

Neither links anything beyond libc and libgcc_s. The container image uses
static musl builds.

## Reading these figures

Server and load generator shared one machine with a desktop session running.
Treat the figures as an order of magnitude and measure your own hardware
before sizing anything. See [run the
benchmarks](/kallisto/how-to/run-benchmarks/).
