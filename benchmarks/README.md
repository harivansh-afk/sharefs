# HTTP download benchmarks

Measured on 2026-10-03, using sharefs 0.5.4 at
`56583198184aa762898106de81af4e85a0b8a675`. This compares the ordinary file GET
path with upstream dufs, nginx, and Python's standard-library file server.

## Method

- NVIDIA DGX Spark, NixOS, Linux 6.17.13, 128 GB unified memory (121 GiB usable).
  Four Cortex-X925 cores at up to 3.9 GHz for each server (CPUs 5–8), and four
  separate Cortex-X925 cores for wrk (15–18). CPU governor: `performance`.
  Other services remained running; affinity is not exclusive CPU reservation.
  Before each trial, the runner waits for less than 0.25 busy cores on the test
  CPUs and less than two busy cores across the host.
  During measurement it subtracts server/client CPU from `/proc/stat` busy time
  and rejects trials with more than 0.25 other busy cores on the eight test CPUs
  or two other busy cores across the host. These counters also include kernel
  activity outside the measured processes; they are a contention check, not exact
  attribution. The first seven accepted trials met a stricter one-core whole-host
  cap; the JSON records this change. Rejected attempts remain in the JSON but are
  excluded from tables.
- Release sharefs from `nix build .#default`; dufs 0.45.0, nginx 1.30.4,
  Python 3.12.13, wrk 4.2.0. Nix packages use the repository's locked nixpkgs
  `c5c4a43b0e8056328ec4529f735cabdb8f1942bb`.
- Plain HTTP/1.1 on IPv4 loopback, persistent connections, no pipelining,
  authentication, TLS, compression, or access logging. No share links configured.
  nginx uses four workers, `sendfile on`, `tcp_nopush on`, and a large keep-alive
  request limit. Python uses `ThreadingHTTPServer` with only HTTP/1.1 and logging
  settings changed; see [python_server.py](python_server.py). Other server
  settings are defaults.
- Two deterministic, non-sparse files: 4 KiB at 16 connections, and 64 MiB at
  8 connections. Both fit in the filesystem page cache. Before each run, two
  complete GETs verify status 200, length, SHA-256, and keep-alive.
- Three trials per server/workload, each with a fresh server, 3-second warmup,
  and 15-second measurement. Server/workload order is shuffled each round with
  a fixed seed. wrk uses four client threads. The short smoke run is excluded.
  An earlier full run was affected by unrelated Rust compilation and is retained
  as [spark-2026-10-03-contended.json](spark-2026-10-03-contended.json); none of its
  numbers are used in the published table.

## Reading the numbers

Throughput, latency, and CPU metrics are per-metric medians of the three accepted
trials. p95 is wrk's latency
percentile for that workload; it describes this closed-loop test, not a latency
guarantee under a fixed arrival rate. Bulk throughput counts completed file
payloads, excluding HTTP headers and unfinished requests at the test boundary.

CPU is server user + system time from `/proc`, including nginx workers and all
threads, excluding the client. 100% means one core; the server allowance is 400%.
CPU seconds per GiB and milliseconds per request divide that time by completed
payload/work. Read them alongside latency: low CPU use can also mean waiting.
Accounting also includes connection cleanup at the measurement boundary.

Memory is process-tree **PSS** (proportional set size): resident memory with shared
pages apportioned, rather than counted once per nginx worker. Idle is the median
across fresh starts, before the file checks and warmup. Peak is the largest sample
across both workloads and all trials, sampled about every 200 ms. Filesystem page
cache and kernel socket buffers are excluded; this is not total system RAM use
or a long-duration leak test.
Raw RSS, CPU time, client CPU, all wrk output, versions, and per-trial data are in
[spark-2026-10-03.json](spark-2026-10-03.json).

## Results

| Server | Workload | Throughput, median (min–max) | p95 ms | Server CPU % | Client CPU % | CPU ms/request | CPU s/GiB |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| sharefs | small | 388.0 (386.9–388.6) req/s | 42.891 | 11.4 | 3.8 | 0.2950 | 77.339 |
| sharefs | bulk | 5,908.5 (5,895.8–5,929.5) MiB/s | 95.614 | 371.7 | 197.3 | 40.2439 | 0.644 |
| dufs | small | 387.4 (385.9–388.8) req/s | 43.069 | 13.0 | 3.7 | 0.3368 | 88.297 |
| dufs | bulk | 5,917.1 (5,904.3–5,950.9) MiB/s | 94.038 | 372.1 | 197.4 | 40.1781 | 0.643 |
| nginx | small | 316,669.5 (315,367.2–317,658.7) req/s | 0.090 | 394.2 | 240.8 | 0.0124 | 3.263 |
| nginx | bulk | 22,989.3 (22,751.4–23,056.1) MiB/s | 35.011 | 133.5 | 398.6 | 3.7051 | 0.059 |
| Python http.server | small | 364.9 (364.7–371.0) req/s | 42.057 | 8.1 | 3.0 | 0.2226 | 58.350 |
| Python http.server | bulk | 7,430.0 (7,407.0–7,445.0) MiB/s | 72.536 | 370.6 | 216.0 | 31.8611 | 0.510 |

All 24 accepted trials completed without request errors; five additional trials
were rejected by the contention checks. sharefs and dufs are effectively tied
on bulk throughput and CPU cost in this test. sharefs used 2.5 MiB idle and
11.0 MiB at its largest sampled peak.

nginx is substantially more efficient for these plain GETs. Its bulk test used
about 399% client CPU, so the throughput is limited by this four-core client; it
is not an estimate of nginx's maximum capacity.

The 4 KiB keep-alive workload shows approximately 43 ms p95 for sharefs and dufs
while most server CPU capacity is idle. This result is specific to this client,
concurrency, and plain HTTP workload; it does not establish browser or HTTPS
latency. Its cause has not been established by this benchmark.

## Practical heuristic

Compare bulk throughput with the connection you need to fill: 1 Gbit/s is at most
119 MiB/s of payload before protocol overhead; 10 Gbit/s is 1,192 MiB/s. Once the
server comfortably clears that target, CPU cost and memory matter more than its
maximum loopback throughput. As a rough sizing estimate, multiply CPU seconds per
GiB by target GiB/s to estimate busy CPU cores. This extrapolates from the tested
workload; TLS, disk reads, concurrency, and real network traffic change the cost.

For many small files, use requests/s and p95 together. For an always-running
service, use idle memory; for simultaneous transfers, use busy peak memory. Compare
feature needs separately: these servers do not offer the same sharing controls,
authentication, upload, or WebDAV behavior.

This test does not measure cold storage, LAN/WAN throughput, HTTPS, uploads,
directory browsing, signed links, many different files, or very high concurrency.

## Reproduce

From the repository root on Linux, with Nix and uv installed:

```sh
nix build .#default --out-link result-benchmark
nix shell --inputs-from . nixpkgs#wrk nixpkgs#nginx nixpkgs#dufs \
  --command uv run --no-project --python 3.12.13 python benchmarks/run.py \
  --sharefs result-benchmark/bin/sharefs \
  --server-cpus 5,6,7,8 --client-cpus 15,16,17,18 \
  --output /tmp/sharefs-bench.json
uv run --no-project python benchmarks/summarize.py /tmp/sharefs-bench.json
uv run --no-project python benchmarks/summarize.py /tmp/sharefs-bench.json --details
```

Select disjoint groups of equivalent physical cores for your machine (`lscpu -e`),
avoiding SMT siblings, and use a new output path. Use `--resume` with the same arguments to continue an interrupted
run; it preserves accepted trials and checks that the setup matches. The runner
otherwise refuses existing result files, binds only to loopback,
removes its temporary files, and terminates its test servers. It needs permission
to read its own processes' `/proc/*/smaps_rollup`. CPU measurements are Linux-only.
