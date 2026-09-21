#!/usr/bin/env python3
"""What Kallisto costs on the machine it runs on: disk, RAM and CPU, idle,
under a sweep of fixed request rates, and saturated.

Server and load generator share one machine, so they are kept apart: the
server on the first half of the logical CPUs, wrk/wrk2 on the second half. The
load generator's own CPU is recorded too, so a run where *it* was the
bottleneck shows up as one rather than as a server ceiling.

CPU and RSS come straight from /proc/<pid>, sampled every half second for the
whole of each phase. CPU is in percent of one logical CPU, so 100 means one
CPU fully busy.

The rate limiter is lifted (as in run_duck_bench.sh): this measures the
serving path, not the token bucket. Everything else is the default
configuration, including the access log, which is written to a file.

Usage: benchmarks/server/resource_profile.py [out.json]
Requires: wrk, wrk2, taskset, a release build of kallisto-server and -ctl.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SERVER = ROOT / "target/release/kallisto-server"
CTL = ROOT / "target/release/kallisto-ctl"
PORT = 8200
URL = f"http://127.0.0.1:{PORT}/v1/secret/data/bench/s0"
WORKERS = 2

CPUS = os.cpu_count() or 2
SERVER_CPUS = f"0-{CPUS // 2 - 1}"
LOADGEN_CPUS = f"{CPUS // 2}-{CPUS - 1}"

IDLE_SECONDS = 60
STEP_SECONDS = 30
SATURATION_SECONDS = 30
SWEEP_RATES = [1_000, 5_000, 10_000, 20_000, 30_000, 40_000, 50_000, 60_000]
TICK = os.sysconf("SC_CLK_TCK")


def cpu_ticks(pid: int) -> int:
    # Fields after the parenthesised name; utime and stime are 14th and 15th.
    stat = Path(f"/proc/{pid}/stat").read_text()
    fields = stat[stat.rindex(")") + 2 :].split()
    return int(fields[11]) + int(fields[12])


def rss_kib(pid: int) -> int:
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    raise RuntimeError(f"no VmRSS for pid {pid}")


def sample(pid: int, seconds: float, interval: float = 0.5) -> list[dict]:
    """CPU (% of one logical CPU) and RSS, every `interval` for `seconds`."""
    samples = []
    last_t, last_ticks = time.monotonic(), cpu_ticks(pid)
    end = last_t + seconds
    while time.monotonic() < end:
        time.sleep(interval)
        now, ticks = time.monotonic(), cpu_ticks(pid)
        cpu = (ticks - last_ticks) / TICK / (now - last_t) * 100
        samples.append({"cpu": cpu, "rss_kib": rss_kib(pid)})
        last_t, last_ticks = now, ticks
    return samples


def summarise(samples: list[dict], skip: int = 0) -> dict:
    """Drops the first `skip` samples (ramp-up) before averaging."""
    kept = samples[skip:] or samples
    cpus = [s["cpu"] for s in kept]
    rss = [s["rss_kib"] for s in kept]
    return {
        "cpu_mean": sum(cpus) / len(cpus),
        "cpu_max": max(cpus),
        "rss_mean_mib": sum(rss) / len(rss) / 1024,
        "rss_max_mib": max(rss) / 1024,
    }


def to_ms(value: str) -> float:
    number, unit = re.fullmatch(r"([\d.]+)(us|ms|s|m)", value).groups()
    return float(number) * {"us": 1e-3, "ms": 1, "s": 1e3, "m": 6e4}[unit]


def parse_wrk(output: str) -> dict:
    rate = float(re.search(r"Requests/sec:\s+([\d.]+)", output).group(1))
    latency = {}
    for pct, value in re.findall(r"^\s+(50|99)(?:\.0+)?%\s+([\d.]+(?:us|ms|s|m))", output, re.M):
        latency[f"p{pct}_ms"] = to_ms(value)
    errors = re.search(r"Non-2xx or 3xx responses:\s+(\d+)", output)
    return {"achieved_rps": rate, **latency, "non_2xx": int(errors.group(1)) if errors else 0}


def run_loadgen(cmd: list[str], seconds: float, server_pid: int) -> tuple[dict, dict, dict]:
    """Runs a load generator pinned to its own CPUs while sampling both it and
    the server. Returns (wrk result, server usage, load generator usage)."""
    proc = subprocess.Popen(
        ["taskset", "-c", LOADGEN_CPUS, *cmd], stdout=subprocess.PIPE, text=True
    )
    loadgen = {}
    watcher = threading.Thread(
        target=lambda: loadgen.update(summarise(sample(proc.pid, seconds - 1), skip=4))
    )
    watcher.start()
    server = summarise(sample(server_pid, seconds - 1), skip=4)
    watcher.join()
    output = proc.communicate()[0]
    return parse_wrk(output), server, loadgen


def seed(workdir: Path) -> dict:
    plain = workdir / "plain.json"
    secrets = {f"bench/s{i}": {"username": f"user{i}", "password": "x" * 32} for i in range(64)}
    plain.write_text(json.dumps({"version": 1, "secrets": secrets, "policies": {}, "tokens": {}}))
    key = subprocess.run([CTL, "gen-key"], capture_output=True, text=True, check=True).stdout.strip()
    env = {**os.environ, "KALLISTO_SEAL_KEY": key}
    subprocess.run(
        [CTL, "seal", "--in", plain, "--out", workdir / "secrets.kal"],
        env=env, check=True, capture_output=True,
    )
    plain.unlink()
    (workdir / "kallisto.yaml").write_text(
        f"""apiVersion: kallisto/v1
kind: Resolver
spec:
  listen:
    port: {PORT}
  workers: {WORKERS}
  source:
    type: disk
    path: {workdir}/secrets.kal
  limits:
    requestsPerSecondPerWorker: 100000000
"""
    )
    return env


def start_server(workdir: Path, env: dict) -> subprocess.Popen:
    server = subprocess.Popen(
        ["taskset", "-c", SERVER_CPUS, SERVER, f"--config={workdir}/kallisto.yaml"],
        stdout=open(workdir / "access.log", "w"),
        stderr=open(workdir / "server.log", "w"),
        env=env,
    )
    for _ in range(80):
        try:
            with urllib.request.urlopen(URL, timeout=1) as response:
                if response.status == 200:
                    return server
        except OSError:
            time.sleep(0.25)
    server.kill()
    raise RuntimeError((workdir / "server.log").read_text())


def footprint() -> dict:
    with tempfile.TemporaryDirectory() as scratch:
        stripped = Path(scratch) / "kallisto-server"
        shutil.copy(SERVER, stripped)
        subprocess.run(["strip", stripped], check=True)
        return {
            "server_bytes": SERVER.stat().st_size,
            "server_stripped_bytes": stripped.stat().st_size,
            "ctl_bytes": CTL.stat().st_size,
        }


def main() -> None:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "resource_profile.json")
    workdir = Path(tempfile.mkdtemp(prefix="kallisto_profile."))
    result = {"footprint": footprint(), "workers": WORKERS,
              "server_cpus": SERVER_CPUS, "loadgen_cpus": LOADGEN_CPUS}
    server = start_server(workdir, seed(workdir))
    try:
        result["startup_rss_mib"] = rss_kib(server.pid) / 1024
        print(f"idle for {IDLE_SECONDS}s...", flush=True)
        result["idle"] = summarise(sample(server.pid, IDLE_SECONDS))

        result["sweep"] = []
        for rate in SWEEP_RATES:
            print(f"wrk2 at {rate} req/s...", flush=True)
            cmd = ["wrk2", "-t2", "-c100", f"-d{STEP_SECONDS}s", f"-R{rate}", "--latency", URL]
            wrk, usage, loadgen = run_loadgen(cmd, STEP_SECONDS, server.pid)
            result["sweep"].append({"target_rps": rate, **wrk, "server": usage, "loadgen": loadgen})

        print(f"saturating with wrk for {SATURATION_SECONDS}s...", flush=True)
        cmd = ["wrk", "-t4", "-c256", f"-d{SATURATION_SECONDS}s", "--latency", URL]
        wrk, usage, loadgen = run_loadgen(cmd, SATURATION_SECONDS, server.pid)
        result["saturated"] = {**wrk, "server": usage, "loadgen": loadgen}

        result["peak_rss_mib"] = int(
            next(l for l in Path(f"/proc/{server.pid}/status").read_text().splitlines()
                 if l.startswith("VmHWM:")).split()[1]
        ) / 1024
    finally:
        server.terminate()
        server.wait()
        shutil.rmtree(workdir)
    out.write_text(json.dumps(result, indent=2))
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
