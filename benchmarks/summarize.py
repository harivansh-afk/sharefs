"""Render tables from run.py's JSON; no benchmark runs or rounding by hand."""

import argparse
import json
from collections import Counter
from pathlib import Path
from statistics import median


def tables(path):
    data = json.loads(path.read_text())
    trials = [t for t in data["trials"] if t.get("accepted", True)]
    servers = ("sharefs", "dufs", "nginx", "python")
    counts = Counter((t["server"], t["workload"]) for t in trials)
    expected = {
        (s, w): data["metadata"]["repeats"] for s in servers for w in ("small", "bulk")
    }
    assert counts == expected, "Incomplete or unexpected trial set"
    assert all(not any(t["errors"].values()) for t in trials), "Request errors"
    compact = [
        "| Server | 4 KiB req/s ↑ | p95 ms ↓ | Bulk MiB/s ↑ | CPU s/GiB ↓ | RAM idle / peak MiB ↓ |",
        "| --- | ---: | ---: | ---: | ---: | ---: |",
    ]
    detailed = [
        "| Server | Workload | Throughput, median (min–max) | p95 ms | Server CPU % | Client CPU % | CPU ms/request | CPU s/GiB |",
        "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    ]
    for server in servers:
        groups = {
            w: [t for t in trials if t["server"] == server and t["workload"] == w]
            for w in ("small", "bulk")
        }

        def med(workload, key, groups=groups):
            return median(t[key] for t in groups[workload])

        memory = [t for t in trials if t["server"] == server]
        label = "Python http.server" if server == "python" else server
        compact.append(
            f"| {label} | {med('small', 'requests_s'):,.0f} | {med('small', 'p95_us') / 1000:.3f} "
            f"| {med('bulk', 'payload_mib_s'):,.0f} | {med('bulk', 'cpu_s_gib'):.3f} "
            f"| {median(t['idle_pss_mib'] for t in memory):.1f} / {max(t['peak_pss_mib'] for t in memory):.1f} |"
        )
        for workload in ("small", "bulk"):
            key = "requests_s" if workload == "small" else "payload_mib_s"
            unit = "req/s" if workload == "small" else "MiB/s"
            values = [t[key] for t in groups[workload]]
            detailed.append(
                f"| {label} | {workload} | {median(values):,.1f} ({min(values):,.1f}–{max(values):,.1f}) {unit} "
                f"| {med(workload, 'p95_us') / 1000:.3f} | {med(workload, 'server_cpu_pct'):.1f} "
                f"| {med(workload, 'client_cpu_pct'):.1f} | {med(workload, 'cpu_ms_request'):.4f} "
                f"| {med(workload, 'cpu_s_gib'):.3f} |"
            )
    return "\n".join(compact), "\n".join(detailed)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("results", type=Path)
    parser.add_argument("--details", action="store_true")
    args = parser.parse_args()
    compact, detailed = tables(args.results)
    print(detailed if args.details else compact)
