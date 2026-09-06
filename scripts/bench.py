#!/usr/bin/env python3
"""Local-only Monitor backend benchmark harness using Python's standard library."""

import argparse
import concurrent.futures
import hashlib
import http.client
import json
import math
import os
import platform
import signal
import socket
import sqlite3
import statistics
import subprocess
import threading
import time
from pathlib import Path


NODE_COUNT = 200


def free_port():
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def wait_for_server(process, port, timeout=10.0):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"monitor-server exited early with {process.returncode}")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                return
        except OSError:
            time.sleep(0.05)
    raise RuntimeError("monitor-server did not begin listening")


def start_server(server, database, log_path):
    port = free_port()
    log_file = open(log_path, "ab", buffering=0)
    process = subprocess.Popen(
        [server, "--listen", f"127.0.0.1:{port}", "--db", str(database)],
        stdin=subprocess.DEVNULL,
        stdout=log_file,
        stderr=log_file,
    )
    try:
        wait_for_server(process, port)
    except Exception:
        stop_server(process)
        log_file.close()
        raise
    return process, port, log_file


def stop_server(process):
    if process.poll() is None:
        process.send_signal(signal.SIGTERM)
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)
            raise RuntimeError("graceful SIGTERM timed out; process required SIGKILL")
    if process.returncode != 0:
        raise RuntimeError(f"monitor-server graceful exit code was {process.returncode}")


def initialize_database(server, database, log_path):
    process, _, log_file = start_server(server, database, log_path)
    try:
        stop_server(process)
    finally:
        log_file.close()
    with sqlite3.connect(database) as connection:
        connection.execute("SELECT count(*) FROM settings").fetchone()


def fixture_tokens(database):
    now = int(time.time())
    tokens = []
    with sqlite3.connect(database) as connection:
        connection.execute("PRAGMA foreign_keys=ON")
        connection.execute("BEGIN")
        for index in range(NODE_COUNT):
            public_id = (index + 1).to_bytes(16, "big").hex()
            raw_token = hashlib.sha256(f"monitor-benchmark-{index}".encode()).digest()
            token_hash = hashlib.sha256(raw_token).digest()
            cursor = connection.execute(
                """INSERT INTO nodes (
                       public_id, name, region_code, sort_order, traffic_limit_bytes,
                       traffic_reset_day, price_micros, currency, renewal_cycle,
                       expires_at, first_seen_at, created_at, updated_at
                   ) VALUES (?, ?, 'US', ?, 1000000000000, 1, NULL, NULL, NULL,
                             NULL, NULL, ?, ?)""",
                (public_id, f"bench-{index:03d}", index, now, now),
            )
            node_id = cursor.lastrowid
            connection.execute(
                "INSERT INTO node_tokens (node_id, token_hash, created_at) VALUES (?, ?, ?)",
                (node_id, token_hash, now),
            )
            connection.execute(
                """INSERT INTO traffic_totals (
                       node_id, rx_total_bytes, tx_total_bytes,
                       last_rx_counter_bytes, last_tx_counter_bytes, last_boot_id, updated_at
                   ) VALUES (?, 0, 0, NULL, NULL, NULL, ?)""",
                (node_id, now),
            )
            tokens.append(raw_token.hex())
        connection.commit()
    return tokens


def report_body(index, sequence):
    rx = 10_000_000 + index * 10_000 + sequence * 4_096
    tx = 5_000_000 + index * 5_000 + sequence * 2_048
    return json.dumps(
        {
            "protocol_version": 1,
            "agent_version": "bench-1",
            "boot_id": f"benchmark-boot-{index}",
            "hostname": f"bench-{index:03d}",
            "os": {
                "name": "Linux",
                "version": "benchmark",
                "kernel": "6.0.0-benchmark",
                "architecture": "x86_64",
                "virtualization": "qemu",
            },
            "cpu": {
                "model": "Benchmark CPU",
                "cores": 1,
                "usage": float((index + sequence) % 100),
                "load_1": 0.1,
                "load_5": 0.2,
                "load_15": 0.3,
            },
            "memory": {
                "total": 1_073_741_824,
                "used": 536_870_912,
                "swap_total": 0,
                "swap_used": 0,
            },
            "disk": {"total": 21_474_836_480, "used": 10_737_418_240},
            "network": {
                "rx_bytes": rx,
                "tx_bytes": tx,
                "rx_rate": 2_048,
                "tx_rate": 1_024,
            },
            "uptime_seconds": 86_400 + sequence,
            "process_count": 32,
            "pings": [],
        },
        separators=(",", ":"),
    )


class Measurements:
    def __init__(self):
        self.lock = threading.Lock()
        self.latencies_ms = []
        self.statuses = {}
        self.network_errors = 0

    def record(self, elapsed, status=None, network_error=False):
        with self.lock:
            self.latencies_ms.append(elapsed * 1_000)
            if network_error:
                self.network_errors += 1
            else:
                self.statuses[status] = self.statuses.get(status, 0) + 1


class ResourceSampler:
    def __init__(self, process, database):
        self.process = process
        self.database = Path(database)
        self.stop_event = threading.Event()
        self.samples = []
        self.peak_wal_bytes = 0
        self.thread = threading.Thread(target=self._run, daemon=True)

    def start(self):
        self._sample()
        self.thread.start()

    def stop(self):
        self.stop_event.set()
        self.thread.join()
        self._sample()

    def _run(self):
        while not self.stop_event.wait(0.5):
            self._sample()

    def _sample(self):
        if self.process.poll() is not None:
            return
        try:
            output = subprocess.check_output(
                ["ps", "-o", "rss=,%cpu=", "-p", str(self.process.pid)],
                text=True,
                stderr=subprocess.DEVNULL,
            ).strip()
            if output:
                rss, cpu = output.split()[:2]
                self.samples.append((time.monotonic(), int(rss), float(cpu)))
        except (OSError, subprocess.SubprocessError, ValueError):
            pass
        wal_path = Path(f"{self.database}-wal")
        if wal_path.exists():
            self.peak_wal_bytes = max(self.peak_wal_bytes, wal_path.stat().st_size)

    def summary(self):
        if not self.samples:
            return {
                "rss_baseline_kib": None,
                "rss_peak_kib": None,
                "rss_end_kib": None,
                "cpu_baseline_percent": None,
                "cpu_peak_percent": None,
            }
        return {
            "rss_baseline_kib": self.samples[0][1],
            "rss_peak_kib": max(sample[1] for sample in self.samples),
            "rss_end_kib": self.samples[-1][1],
            "cpu_baseline_percent": self.samples[0][2],
            "cpu_peak_percent": max(sample[2] for sample in self.samples),
        }


def percentile(values, fraction):
    if not values:
        return None
    ordered = sorted(values)
    position = max(0, math.ceil(len(ordered) * fraction) - 1)
    return round(ordered[position], 3)


def measurement_summary(measurements, duration, expected_status):
    requested = len(measurements.latencies_ms)
    successes = measurements.statuses.get(expected_status, 0)
    non_expected = sum(measurements.statuses.values()) - successes
    return {
        "requested": requested,
        "success": successes,
        "non_expected_status": non_expected,
        "network_errors": measurements.network_errors,
        "achieved_rps": round(requested / duration, 2),
        "p50_ms": percentile(measurements.latencies_ms, 0.50),
        "p95_ms": percentile(measurements.latencies_ms, 0.95),
        "p99_ms": percentile(measurements.latencies_ms, 0.99),
    }


def send_reports(port, token, index, target_rps, duration, measurements):
    interval = NODE_COUNT / target_rps
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    sequence = 2
    deadline = time.monotonic() + duration
    next_send = time.monotonic() + (index / NODE_COUNT) * interval
    try:
        while next_send < deadline:
            delay = next_send - time.monotonic()
            if delay > 0:
                time.sleep(delay)
            sequence += 1
            body = report_body(index, sequence)
            started = time.monotonic()
            try:
                connection.request(
                    "POST",
                    "/api/agent/report",
                    body=body,
                    headers={
                        "Authorization": f"Bearer {token}",
                        "Content-Type": "application/json",
                    },
                )
                response = connection.getresponse()
                response.read()
                measurements.record(time.monotonic() - started, response.status)
            except (OSError, http.client.HTTPException):
                measurements.record(time.monotonic() - started, network_error=True)
                connection.close()
                connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
            next_send += interval
    finally:
        connection.close()


def database_stats(database, peak_wal_bytes):
    with sqlite3.connect(database) as connection:
        page_count = connection.execute("PRAGMA page_count").fetchone()[0]
        page_size = connection.execute("PRAGMA page_size").fetchone()[0]
    return {
        "database_bytes": Path(database).stat().st_size,
        "wal_peak_bytes": peak_wal_bytes,
        "page_count": page_count,
        "page_size": page_size,
    }


def classify(result, target_rps):
    if result["network_errors"] or result["non_expected_status"]:
        return "FAILED TARGET"
    if result["achieved_rps"] < target_rps * 0.9:
        return "CLIENT LIMITED"
    return "DIAGNOSTIC BASELINE"


def run_report_scenario(server, root, name, target_rps, duration):
    directory = root / name
    directory.mkdir()
    database = directory / "monitor.db"
    log = directory / "server.log"
    initialize_database(server, database, log)
    tokens = fixture_tokens(database)
    process, port, log_file = start_server(server, database, log)
    sampler = ResourceSampler(process, database)
    measurements = Measurements()
    sampler_started = False
    try:
        seed_live_snapshots(port, tokens)
        time.sleep(0.5)
        started = time.monotonic()
        sampler.start()
        sampler_started = True
        with concurrent.futures.ThreadPoolExecutor(max_workers=NODE_COUNT) as executor:
            futures = [
                executor.submit(
                    send_reports,
                    port,
                    tokens[index],
                    index,
                    target_rps,
                    duration,
                    measurements,
                )
                for index in range(NODE_COUNT)
            ]
            for future in futures:
                future.result()
        actual_duration = time.monotonic() - started
    finally:
        if sampler_started:
            sampler.stop()
        stop_server(process)
        log_file.close()
    result = measurement_summary(measurements, actual_duration, 204)
    result.update(sampler.summary())
    result.update(database_stats(database, sampler.peak_wal_bytes))
    result["target_rps"] = target_rps
    result["duration_seconds"] = round(actual_duration, 3)
    result["classification"] = classify(result, target_rps)
    return result


def request_once(port, method, path, headers=None, body=None):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
    try:
        connection.request(method, path, body=body, headers=headers or {})
        response = connection.getresponse()
        data = response.read()
        return response.status, {key.lower(): value for key, value in response.getheaders()}, data
    finally:
        connection.close()


def seed_live_snapshots(port, tokens):
    def seed(index):
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
        try:
            for sequence in (1, 2):
                body = report_body(index, sequence)
                connection.request(
                    "POST",
                    "/api/agent/report",
                    body=body,
                    headers={
                        "Authorization": f"Bearer {tokens[index]}",
                        "Content-Type": "application/json",
                    },
                )
                response = connection.getresponse()
                response.read()
                if response.status != 204:
                    raise RuntimeError(f"seed report returned {response.status}")
        finally:
            connection.close()

    with concurrent.futures.ThreadPoolExecutor(max_workers=NODE_COUNT) as executor:
        list(executor.map(seed, range(NODE_COUNT)))


def status_worker(port, deadline, measurements):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    try:
        while time.monotonic() < deadline:
            started = time.monotonic()
            try:
                connection.request("GET", "/api/public/snapshot")
                response = connection.getresponse()
                response.read()
                measurements.record(time.monotonic() - started, response.status)
            except (OSError, http.client.HTTPException):
                measurements.record(time.monotonic() - started, network_error=True)
                connection.close()
                connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
    finally:
        connection.close()


def run_status(server, seconds, concurrency, root):
    database = root / "monitor.db"
    log = root / "server.log"
    initialize_database(server, database, log)
    tokens = fixture_tokens(database)
    process, port, log_file = start_server(server, database, log)
    sampler = ResourceSampler(process, database)
    try:
        seed_live_snapshots(port, tokens)
        time.sleep(2.2)
        status, headers, body = request_once(port, "GET", "/api/public/snapshot")
        if status != 200:
            raise RuntimeError(f"snapshot fixture returned {status}")
        etag = headers.get("etag")
        conditional_status, _, conditional_body = request_once(
            port,
            "GET",
            "/api/public/snapshot",
            headers={"If-None-Match": etag},
        )
        if conditional_status != 304 or conditional_body:
            raise RuntimeError("snapshot ETag did not return an empty 304")

        measurements = Measurements()
        deadline = time.monotonic() + seconds
        sampler.start()
        started = time.monotonic()
        with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
            futures = [
                executor.submit(status_worker, port, deadline, measurements)
                for _ in range(concurrency)
            ]
            for future in futures:
                future.result()
        actual_duration = time.monotonic() - started
        sampler.stop()
        result = measurement_summary(measurements, actual_duration, 200)
        result.update(sampler.summary())
        result.update(database_stats(database, sampler.peak_wal_bytes))
        result["duration_seconds"] = round(actual_duration, 3)
        result["concurrency"] = concurrency
        result["response_bytes"] = len(body)
        result["etag_304_verified"] = True
        result["classification"] = (
            "FAILED TARGET"
            if result["network_errors"] or result["non_expected_status"]
            else "DIAGNOSTIC BASELINE"
        )
        return result
    finally:
        if sampler.thread.is_alive():
            sampler.stop()
        stop_server(process)
        log_file.close()


def environment():
    return {
        "os": platform.platform(),
        "architecture": platform.machine(),
        "cpu": platform.processor() or "unknown",
        "python": platform.python_version(),
        "note": "Python stdlib client overhead is included; macOS data is diagnostic, not Linux acceptance data.",
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("mode", choices=("report", "status"))
    parser.add_argument("--server", required=True)
    parser.add_argument("--work-dir", required=True)
    parser.add_argument("--steady-seconds", type=float, default=60)
    parser.add_argument("--stress-seconds", type=float, default=30)
    parser.add_argument("--seconds", type=float, default=30)
    parser.add_argument("--concurrency", type=int, default=32)
    arguments = parser.parse_args()
    server = str(Path(arguments.server).resolve())
    if not os.access(server, os.X_OK):
        raise SystemExit(f"monitor-server is not executable: {server}")
    root = Path(arguments.work_dir).resolve()
    if not root.is_dir():
        raise SystemExit(f"benchmark work directory does not exist: {root}")

    output = {"environment": environment()}
    if arguments.mode == "report":
        output["200_nodes_2_seconds"] = run_report_scenario(
            server, root, "steady", 100.0, arguments.steady_seconds
        )
        output["1000_reports_per_second"] = run_report_scenario(
            server, root, "stress", 1_000.0, arguments.stress_seconds
        )
    else:
        output["public_snapshot_200"] = run_status(
            server, arguments.seconds, arguments.concurrency, root
        )
    print(json.dumps(output, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
