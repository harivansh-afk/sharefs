"""Linux loopback GET benchmark; requires wrk, nginx, dufs and a release sharefs."""

import argparse
import hashlib
import http.client
import json
import os
import platform
import random
import shutil
import signal
import socket
import statistics
import subprocess
import sys
import tempfile
import time
from datetime import datetime, timezone
from pathlib import Path

MIB = 1024**2
WORKLOADS = [("small", 4096, 16), ("bulk", 64 * MIB, 8)]


def cpu_busy():
    result = {}
    for line in Path("/proc/stat").read_text().splitlines():
        fields = line.split()
        if fields[0].startswith("cpu"):
            values = list(map(int, fields[1:]))
            # user, nice, system, IRQ, softIRQ, steal; exclude idle/iowait.
            result[fields[0]] = sum(values[i] for i in (0, 1, 2, 5, 6, 7)) / os.sysconf(
                "SC_CLK_TCK"
            )
    return result


def wait_quiet(cpus):
    deadline = time.monotonic() + 600
    checks = 0
    stable = 0
    while time.monotonic() < deadline:
        before, started = cpu_busy(), time.monotonic()
        time.sleep(1)
        after = cpu_busy()
        elapsed = time.monotonic() - started
        busy_cores = (after["cpu"] - before["cpu"]) / elapsed
        test_cores = (
            sum(after[f"cpu{cpu}"] - before[f"cpu{cpu}"] for cpu in cpus) / elapsed
        )
        stable = stable + 1 if busy_cores < 2.0 and test_cores < 0.25 else 0
        if stable >= 3:
            return
        if checks % 15 == 0:
            print(
                f"Waiting for a quiet host ({busy_cores:.1f} busy cores)...", flush=True
            )
        checks += 1
    raise RuntimeError("Host remained busy for 10 minutes; retry later")


def cpu_set(value):
    return {int(cpu) for cpu in value.split(",")}


def spawn(command, cpus, **kwargs):
    # taskset applies affinity before runtime/worker initialization.
    return subprocess.Popen(
        ["taskset", "-c", ",".join(map(str, sorted(cpus))), *command],
        start_new_session=True,
        **kwargs,
    )


def stop(process):
    if process.poll() is None:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()


def process_tree(pid):
    """These servers spawn child processes on the main thread (nginx workers)."""
    yield pid
    try:
        children = Path(f"/proc/{pid}/task/{pid}/children").read_text().split()
    except FileNotFoundError:
        return
    for child in children:
        yield from process_tree(int(child))


def resources(pid):
    ticks, pss_kib, rss_kib = {}, 0, 0
    for child in process_tree(pid):
        try:
            stat = Path(f"/proc/{child}/stat").read_text().rsplit(")", 1)[1].split()
            ticks[child] = int(stat[11]) + int(stat[12])  # process user + system
            for line in Path(f"/proc/{child}/smaps_rollup").read_text().splitlines():
                if line.startswith("Pss:"):
                    pss_kib += int(line.split()[1])
                elif line.startswith("Rss:"):
                    rss_kib += int(line.split()[1])
        except (FileNotFoundError, ProcessLookupError):
            continue
    return ticks, pss_kib / 1024, rss_kib / 1024


def wait_ready(process, port):
    for _ in range(100):
        if process.poll() is not None:
            raise RuntimeError("Server exited; see its log")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                return
        except OSError:
            time.sleep(0.05)
    raise RuntimeError("Server did not listen")


def verify(port, name, size, digest):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
    # Two requests on the same connection verify both bytes and keep-alive.
    for _ in range(2):
        connection.request("GET", f"/{name}.bin")
        response = connection.getresponse()
        body = response.read()
        assert response.status == 200, response.status
        assert not response.will_close, "Expected HTTP keep-alive"
        assert len(body) == size and hashlib.sha256(body).hexdigest() == digest
    connection.close()


def wrk_command(args, port, name, connections, seconds, lua):
    return [
        "wrk",
        "-t",
        str(min(4, len(args.client_cpus), connections)),
        "-c",
        str(connections),
        "-d",
        f"{seconds}s",
        "--timeout",
        "10s",
        "--latency",
        "-s",
        str(lua),
        f"http://127.0.0.1:{port}/{name}.bin",
    ]


def parse_wrk(output):
    return json.loads(
        next(line[7:] for line in output.splitlines() if line.startswith("RESULT "))
    )


def run_load(command, cpus, server):
    before, _, _ = resources(server.pid)
    host_before = cpu_busy()
    last = dict(before)
    peak_pss, peak_rss, samples = 0, 0, 0
    with tempfile.TemporaryFile(mode="w+") as log:
        client = spawn(command, cpus, stdout=log, stderr=subprocess.STDOUT)
        started = time.monotonic()
        try:
            while client.poll() is None:
                ticks, pss, rss = resources(server.pid)
                last.update(ticks)
                peak_pss, peak_rss = max(peak_pss, pss), max(peak_rss, rss)
                samples += 1
                time.sleep(0.2)
            # The caller uses RUSAGE_CHILDREN deltas for this sole reaped child.
            wall = time.monotonic() - started
            ticks, _, _ = resources(server.pid)
            last.update(ticks)
            host_after = cpu_busy()
            if client.returncode != 0 or server.poll() is not None:
                raise RuntimeError("Load generator or server exited unexpectedly")
            log.seek(0)
            output = log.read()
        finally:
            stop(client)
    data = parse_wrk(output)
    if any(data["errors"].values()) or not data["requests"]:
        raise RuntimeError(f"Failed requests: {output}")
    cpu = sum(value - before.get(pid, 0) for pid, value in last.items()) / os.sysconf(
        "SC_CLK_TCK"
    )
    return dict(
        **data,
        server_cpu_s=cpu,
        sampled_wall_s=wall,
        peak_pss_mib=peak_pss,
        peak_rss_mib=peak_rss,
        memory_samples=samples,
        wrk_output=output,
        host_cpu_busy_s={
            key: host_after[key] - value for key, value in host_before.items()
        },
    )


def trial(args, base, server_name, workload, repeat, digests):
    import resource

    name, size, connections = workload
    root = base / "files"
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
    config = base / "nginx.conf"
    config.write_text(f"""daemon off;
worker_processes {len(args.server_cpus)};
pid {base}/nginx.pid;
error_log stderr error;
events {{ worker_connections 1024; }}
http {{
    access_log off;
    sendfile on;
    tcp_nopush on;
    gzip off;
    keepalive_requests 1000000;
    server {{ listen 127.0.0.1:{port}; root {root}; }}
}}
""")
    commands = {
        "sharefs": [
            args.sharefs,
            str(root),
            "--bind",
            "127.0.0.1",
            "--port",
            str(port),
            "--log-format",
            "",
        ],
        "dufs": [
            "dufs",
            str(root),
            "--bind",
            "127.0.0.1",
            "--port",
            str(port),
            "--log-format",
            "",
        ],
        "nginx": ["nginx", "-p", str(base), "-c", str(config)],
        "python": [
            sys.executable,
            str(Path(__file__).with_name("python_server.py").resolve()),
            str(root),
            str(port),
        ],
    }
    with tempfile.TemporaryFile(mode="w+") as log:
        server = spawn(
            commands[server_name],
            args.server_cpus,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        try:
            wait_ready(server, port)
            time.sleep(1)
            idle_pss = statistics.median(resources(server.pid)[1] for _ in range(3))
            verify(port, name, size, digests[name])
            warmup = wrk_command(
                args, port, name, connections, args.warmup, base / "stats.lua"
            )
            warm = spawn(
                warmup,
                args.client_cpus,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
            )
            try:
                warm_output = warm.communicate(timeout=args.warmup + 15)[0].decode()
            finally:
                stop(warm)
            if warm.returncode != 0 or any(parse_wrk(warm_output)["errors"].values()):
                raise RuntimeError(f"Warmup failed: {warm_output}")
            time.sleep(0.2)
            command = wrk_command(
                args, port, name, connections, args.seconds, base / "stats.lua"
            )
            before = resource.getrusage(resource.RUSAGE_CHILDREN)
            result = run_load(command, args.client_cpus, server)
            after = resource.getrusage(resource.RUSAGE_CHILDREN)
            result["client_cpu_s"] = (
                after.ru_utime + after.ru_stime - before.ru_utime - before.ru_stime
            )
            measured_cpu = result["server_cpu_s"] + result["client_cpu_s"]
            group_cpu = sum(
                result["host_cpu_busy_s"][f"cpu{cpu}"]
                for cpu in args.server_cpus | args.client_cpus
            )
            result["other_cpu_cores_on_test_cpus"] = (
                max(0, group_cpu - measured_cpu) / result["sampled_wall_s"]
            )
            result["other_cpu_cores_host"] = (
                max(0, result["host_cpu_busy_s"]["cpu"] - measured_cpu)
                / result["sampled_wall_s"]
            )
            result["accepted"] = (
                result["other_cpu_cores_on_test_cpus"] <= 0.25
                and result["other_cpu_cores_host"] <= 2.0
            )
            seconds = result["duration_us"] / 1e6
            result.update(
                server=server_name,
                workload=name,
                repeat=repeat,
                file_bytes=size,
                connections=connections,
                idle_pss_mib=idle_pss,
                requests_s=result["requests"] / seconds,
                payload_mib_s=result["requests"] * size / MIB / seconds,
                server_cpu_pct=result["server_cpu_s"] / seconds * 100,
                client_cpu_pct=result["client_cpu_s"] / seconds * 100,
                cpu_ms_request=result["server_cpu_s"] * 1000 / result["requests"],
                cpu_s_gib=result["server_cpu_s"]
                / (result["requests"] * size / 1024**3),
            )
            return result
        except BaseException:
            log.seek(0)
            print(log.read(), file=sys.stderr)
            raise
        finally:
            stop(server)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sharefs", required=True)
    parser.add_argument("--server-cpus", type=cpu_set, required=True)
    parser.add_argument("--client-cpus", type=cpu_set, required=True)
    parser.add_argument("--seconds", type=int, default=15)
    parser.add_argument("--warmup", type=int, default=3)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument(
        "--resume",
        action="store_true",
        help="Keep accepted trials and resume an interrupted run",
    )
    args = parser.parse_args()
    assert args.seconds > 0 and args.warmup > 0 and args.repeats > 0
    assert (
        args.server_cpus
        and args.client_cpus
        and not args.server_cpus & args.client_cpus
    )
    assert args.server_cpus | args.client_cpus <= os.sched_getaffinity(0)
    args.sharefs = str(Path(args.sharefs).resolve())
    for executable in (args.sharefs, "wrk", "nginx", "dufs", "taskset"):
        if not shutil.which(executable):
            parser.error(f"Missing executable: {executable}")
    if args.resume:
        existing = json.loads(args.output.read_text())
    else:
        assert not args.output.exists(), (
            "Choose a new output path, or explicitly --resume"
        )
        existing = None
    metadata = {
        "date_utc": datetime.now(timezone.utc).isoformat(),
        "platform": platform.platform(),
        "python": sys.version,
        "commit": subprocess.check_output(
            ["git", "rev-parse", "HEAD"], text=True
        ).strip(),
        "lscpu": subprocess.check_output(["lscpu"], text=True),
        "cpu_topology": subprocess.check_output(
            ["lscpu", "-e=CPU,CORE,MAXMHZ,MINMHZ"], text=True
        ),
        "meminfo": Path("/proc/meminfo").read_text(),
        "loadavg_before": Path("/proc/loadavg").read_text().strip(),
        "server_cpus": sorted(args.server_cpus),
        "client_cpus": sorted(args.client_cpus),
        "seconds": args.seconds,
        "warmup_seconds": args.warmup,
        "repeats": args.repeats,
        "sample_interval_s": 0.2,
        "versions": {},
        "contention_limits_cores": {"test_cpus": 0.25, "whole_host": 2.0},
        "nixpkgs": json.loads(Path("flake.lock").read_text())["nodes"]["nixpkgs"][
            "locked"
        ],
    }
    for name, command in {
        "sharefs": [args.sharefs, "--version"],
        "dufs": ["dufs", "--version"],
        "nginx": ["nginx", "-v"],
        "wrk": ["wrk", "--version"],
    }.items():
        version = subprocess.run(command, capture_output=True, text=True, check=False)
        metadata["versions"][name] = (version.stdout + version.stderr).splitlines()[0]
    if existing:
        for key in (
            "commit",
            "python",
            "server_cpus",
            "client_cpus",
            "seconds",
            "warmup_seconds",
            "repeats",
            "versions",
            "nixpkgs",
            "contention_limits_cores",
        ):
            assert existing["metadata"][key] == metadata[key], (
                f"Cannot resume: {key} changed"
            )
        results = existing
        metadata = results["metadata"]
        metadata.setdefault("resumed_utc", []).append(
            datetime.now(timezone.utc).isoformat()
        )
    else:
        results = {"metadata": metadata, "trials": []}
    with tempfile.TemporaryDirectory(prefix="sharefs-bench-") as directory:
        base = Path(directory)
        (base / "files").mkdir()
        digests = {}
        for name, size, _ in WORKLOADS:
            data = random.Random(42).randbytes(size)
            (base / "files" / f"{name}.bin").write_bytes(data)
            digests[name] = hashlib.sha256(data).hexdigest()
            del data
        metadata["sha256"] = digests
        (base / "stats.lua").write_text("""done = function(s, latency, requests)
  io.write(string.format('RESULT {"duration_us":%d,"requests":%d,"wire_bytes":%d,"p50_us":%d,"p95_us":%d,"p99_us":%d,"errors":{"connect":%d,"read":%d,"write":%d,"status":%d,"timeout":%d}}\\n',
    s.duration, s.requests, s.bytes, latency:percentile(50), latency:percentile(95), latency:percentile(99),
    s.errors.connect, s.errors.read, s.errors.write, s.errors.status, s.errors.timeout))
end
""")
        for repeat in range(1, args.repeats + 1):
            order = [
                (server, work)
                for server in ("sharefs", "dufs", "nginx", "python")
                for work in WORKLOADS
            ]
            random.Random(42 + repeat).shuffle(order)
            for server, work in order:
                previous = [
                    t
                    for t in results["trials"]
                    if (t["repeat"], t["server"], t["workload"])
                    == (repeat, server, work[0])
                ]
                if any(t["accepted"] for t in previous):
                    continue
                for attempt in range(1, 6):
                    wait_quiet(args.server_cpus | args.client_cpus)
                    result = trial(args, base, server, work, repeat, digests)
                    result["attempt"] = len(previous) + attempt
                    result["finished_utc"] = datetime.now(timezone.utc).isoformat()
                    results["trials"].append(result)
                    args.output.write_text(json.dumps(results, indent=2) + "\n")
                    print(
                        f"{repeat}/{args.repeats} {server:7} {work[0]:5}: {result['requests_s']:9.1f} req/s, "
                        f"{result['payload_mib_s']:8.1f} MiB/s, {result['server_cpu_pct']:6.1f}% CPU, "
                        f"{result['peak_pss_mib']:5.1f} MiB PSS; accepted={result['accepted']}; "
                        f"other cores={result['other_cpu_cores_on_test_cpus']:.2f}/{result['other_cpu_cores_host']:.2f}",
                        flush=True,
                    )
                    if result["accepted"]:
                        break
                else:
                    raise RuntimeError(
                        "Five contended attempts; retry on a quieter host"
                    )
    metadata["loadavg_after"] = Path("/proc/loadavg").read_text().strip()
    args.output.write_text(json.dumps(results, indent=2) + "\n")


if __name__ == "__main__":
    main()
