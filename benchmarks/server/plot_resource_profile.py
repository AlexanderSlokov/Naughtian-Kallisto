#!/usr/bin/env python3
"""Turns resource_profile.py runs into the median table and the load-vs-resource
chart that README.md shows.

Takes the median of every number across runs, so one noisy run on a desktop
machine does not become the published figure. Writes two files:

    <out>.json  the medians, which the README table is copied from
    <out>.svg   CPU and RSS against request rate, as two panels

Two panels rather than one chart with two y-axes: CPU and RAM have unrelated
scales, and a second axis invites reading a crossing point that means nothing.

Usage: benchmarks/server/plot_resource_profile.py <out-stem> run1.json [run2.json ...]
Standard library only.
"""

import json
import statistics
import sys
from pathlib import Path


def median_of(values: list):
    """Median of numbers, or of dicts of numbers field by field."""
    first = values[0]
    if isinstance(first, dict):
        return {key: median_of([v[key] for v in values]) for key in first}
    if isinstance(first, (int, float)):
        return statistics.median(values)
    return first


def combine(runs: list[dict]) -> dict:
    combined = {key: median_of([r[key] for r in runs]) for key in runs[0] if key != "sweep"}
    combined["sweep"] = [median_of(list(step)) for step in zip(*(r["sweep"] for r in runs))]
    combined["runs"] = len(runs)
    return combined


# Chart geometry and tokens. Light and dark come from the same ramps; the dark
# values are selected for the dark surface, not inverted.
WIDTH, HEIGHT = 880, 360
PANEL_W, PANEL_H = 360, 220
TOP, LEFT_PAD = 70, 64
STYLE = """
  .surface { fill: #fcfcfb; }
  .ink { fill: #0b0b0b; }
  .ink-2 { fill: #52514e; }
  .grid { stroke: #e4e3df; stroke-width: 1; }
  .axis { stroke: #b9b8b2; stroke-width: 1; }
  .ref { stroke: #8a8984; stroke-width: 1; }
  .series { stroke: #2a78d6; stroke-width: 2; fill: none; stroke-linejoin: round; stroke-linecap: round; }
  .dot { fill: #2a78d6; stroke: #fcfcfb; stroke-width: 2; }
  .dot-open { fill: #fcfcfb; stroke: #2a78d6; stroke-width: 2; }
  text { font-family: -apple-system, "Segoe UI", Helvetica, Arial, sans-serif; }
  @media (prefers-color-scheme: dark) {
    .surface { fill: #1a1a19; }
    .ink { fill: #ffffff; }
    .ink-2 { fill: #c3c2b7; }
    .grid { stroke: #33332f; }
    .axis { stroke: #5c5b56; }
    .ref { stroke: #8a8984; }
    .series { stroke: #3987e5; }
    .dot { fill: #3987e5; stroke: #1a1a19; }
    .dot-open { fill: #1a1a19; stroke: #3987e5; }
  }
"""


def panel(x0: int, title: str, unit: str, suffix: str, y_max: float, y_step: float,
          points: list[tuple[float, float]], saturated: tuple[float, float],
          x_max: float, reference: tuple[float, str] | None) -> str:
    """One small multiple: the sweep as a line, idle as its first point, and
    the closed-loop saturated run as an open marker off the line."""
    def sx(v: float) -> float:
        return x0 + v / x_max * PANEL_W

    def sy(v: float) -> float:
        return TOP + PANEL_H - v / y_max * PANEL_H

    parts = [f'<text class="ink" x="{x0}" y="{TOP - 30}" font-size="14" font-weight="600">{title}</text>',
             f'<text class="ink-2" x="{x0}" y="{TOP - 12}" font-size="12">{unit}</text>']
    tick = 0.0
    while tick <= y_max + 1e-9:
        y = sy(tick)
        parts.append(f'<line class="grid" x1="{x0}" x2="{x0 + PANEL_W}" y1="{y:.1f}" y2="{y:.1f}"/>')
        parts.append(f'<text class="ink-2" x="{x0 - 8}" y="{y + 4:.1f}" font-size="11" text-anchor="end">{tick:g}</text>')
        tick += y_step
    for rate in range(0, int(x_max) + 1, 20_000):
        label = "0" if rate == 0 else f"{rate // 1000}k"
        parts.append(f'<text class="ink-2" x="{sx(rate):.1f}" y="{TOP + PANEL_H + 18}" font-size="11" text-anchor="middle">{label}</text>')
    parts.append(f'<line class="axis" x1="{x0}" x2="{x0 + PANEL_W}" y1="{sy(0):.1f}" y2="{sy(0):.1f}"/>')
    parts.append(f'<text class="ink-2" x="{x0 + PANEL_W / 2}" y="{TOP + PANEL_H + 38}" font-size="12" text-anchor="middle">requests per second</text>')

    if reference:
        value, label = reference
        parts.append(f'<line class="ref" x1="{x0}" x2="{x0 + PANEL_W}" y1="{sy(value):.1f}" y2="{sy(value):.1f}"/>')
        parts.append(f'<text class="ink-2" x="{x0 + 4}" y="{sy(value) - 5:.1f}" font-size="11">{label}</text>')

    path = " ".join(f"{sx(x):.1f},{sy(y):.1f}" for x, y in points)
    parts.append(f'<polyline class="series" points="{path}"/>')
    for x, y in points:
        parts.append(f'<circle class="dot" cx="{sx(x):.1f}" cy="{sy(y):.1f}" r="4"><title>{x:,.0f} req/s: {y:.1f}</title></circle>')
    sat_x, sat_y = saturated
    parts.append(f'<circle class="dot-open" cx="{sx(sat_x):.1f}" cy="{sy(sat_y):.1f}" r="5"><title>saturated, {sat_x:,.0f} req/s: {sat_y:.1f}</title></circle>')

    # The line leaves the idle point steeply upwards, so the label goes to the
    # right and below it, kept above the x-axis, where the line never is.
    idle_x, idle_y = points[0]
    label_y = min(sy(idle_y) + 14, sy(0) - 4)
    parts.append(f'<text class="ink" x="{sx(idle_x) + 14:.1f}" y="{label_y:.1f}" font-size="11">idle {idle_y:.1f}{suffix}</text>')
    parts.append(f'<text class="ink" x="{sx(sat_x) - 8:.1f}" y="{sy(sat_y) - 10:.1f}" font-size="11" text-anchor="end">saturated {sat_y:.1f}{suffix}</text>')
    return "\n".join(parts)


def render(data: dict) -> str:
    sweep, sat, idle = data["sweep"], data["saturated"], data["idle"]
    x_max = 70_000
    cpu = [(0, idle["cpu_mean"])] + [(s["achieved_rps"], s["server"]["cpu_mean"]) for s in sweep]
    ram = [(0, idle["rss_max_mib"])] + [(s["achieved_rps"], s["server"]["rss_max_mib"]) for s in sweep]
    body = [
        panel(LEFT_PAD, "CPU", "percent of one logical CPU, mean", "%", 250, 50, cpu,
              (sat["achieved_rps"], sat["server"]["cpu_mean"]), x_max,
              (200, "200% = both worker CPUs busy")),
        panel(LEFT_PAD + PANEL_W + 90, "Memory", "resident set (RSS), MiB, peak", " MiB", 20, 5, ram,
              (sat["achieved_rps"], sat["server"]["rss_max_mib"]), x_max, None),
    ]
    note = ("Filled points: wrk2 at a fixed rate, 100 connections. Open point: wrk, "
            f"256 connections, as fast as it goes. Median of {data['runs']} runs.")
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WIDTH} {HEIGHT}" width="{WIDTH}" height="{HEIGHT}" role="img" aria-labelledby="t d">
<title id="t">Kallisto resource use against request rate</title>
<desc id="d">Two panels. CPU rises from {idle['cpu_mean']:.1f} percent of one logical CPU idle to {sat['server']['cpu_mean']:.0f} percent saturated at {sat['achieved_rps']:,.0f} requests per second. Resident memory stays between {idle['rss_max_mib']:.1f} and {sat['server']['rss_max_mib']:.1f} MiB.</desc>
<style>{STYLE}</style>
<rect class="surface" width="{WIDTH}" height="{HEIGHT}" rx="8"/>
{chr(10).join(body)}
<text class="ink-2" x="{LEFT_PAD}" y="{HEIGHT - 14}" font-size="11">{note}</text>
</svg>
"""


def main() -> None:
    stem = Path(sys.argv[1])
    data = combine([json.loads(Path(p).read_text()) for p in sys.argv[2:]])
    stem.with_suffix(".json").write_text(json.dumps(data, indent=2))
    stem.with_suffix(".svg").write_text(render(data))
    print(f"wrote {stem.with_suffix('.json')} and {stem.with_suffix('.svg')}")


if __name__ == "__main__":
    main()
