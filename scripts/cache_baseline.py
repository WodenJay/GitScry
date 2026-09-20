#!/usr/bin/env python3
"""Measure GitScry's release cache and ordinary-command baseline.

The harness deliberately stays outside the Rust test suite: cache size, process memory,
and filesystem-cache state are host measurements, not CLI behavior assertions.
"""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import math
import os
import platform
import shutil
import sqlite3
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


MIN_RUNS = 5
DEFAULT_TIMEOUT_SECONDS = 900

# These are the two repositories selected in docs/research/validation-repositories.md.
DEFAULT_PROBES = {
    "medium": "gateway/run.py",
    "large": "library/alloc/src/rc.rs",
}

# Approximate pre-optimization measurements recorded in issue #21. They are included beside
# each run so a report cannot accidentally lose the before comparison when the cache changes.
REFERENCE_BASELINE = {
    "source": "#21",
    "large_repository": "38,306 commits",
    "cache_bytes": 1_450_000_000,
    "cache_objects": {
        "commits": "731.8 MiB",
        "search_fts": "362.0 MiB",
        "hunks": "174.8 MiB",
    },
    "query_preparation_seconds": 4.5,
    "known_slow_commands_seconds": {
        "failures": 31.8,
        "related_cold": 61.5,
        "tests_cold": 13.1,
    },
}


class BenchmarkError(RuntimeError):
    pass


@dataclass(frozen=True)
class CommandSpec:
    name: str
    args: tuple[str, ...]


@dataclass(frozen=True)
class ProcessResult:
    returncode: int
    stdout: bytes
    stderr: bytes
    wall_seconds: float
    peak_rss_bytes: int | None


def parse_assignment(value: str, option: str) -> tuple[str, str]:
    name, separator, path = value.partition("=")
    if not separator or not name or not path:
        raise BenchmarkError(f"{option} must be NAME=PATH: {value}")
    return name, path


def run_process(argv: Iterable[str | Path], cwd: Path, timeout: float) -> ProcessResult:
    command = [str(argument) for argument in argv]
    started = time.perf_counter()
    process = subprocess.Popen(
        command,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    peak_rss = 0
    stdout = b""
    stderr = b""
    deadline = started + timeout
    while True:
        current_rss = read_peak_rss(process.pid)
        if current_rss is not None:
            peak_rss = max(peak_rss, current_rss)
        remaining = deadline - time.perf_counter()
        if remaining <= 0:
            process.kill()
            process.communicate()
            raise BenchmarkError(
                f"command exceeded {timeout:g}s: {' '.join(command)}"
            )
        try:
            stdout, stderr = process.communicate(timeout=min(0.05, remaining))
            break
        except subprocess.TimeoutExpired:
            continue
    current_rss = read_peak_rss(process.pid)
    if current_rss is not None:
        peak_rss = max(peak_rss, current_rss)
    elapsed = time.perf_counter() - started
    return ProcessResult(
        process.returncode,
        stdout,
        stderr,
        elapsed,
        peak_rss or None,
    )


def read_peak_rss(pid: int) -> int | None:
    if os.name == "nt":
        return read_windows_peak_rss(pid)
    status = Path(f"/proc/{pid}/status")
    if status.exists():
        try:
            values = {}
            for line in status.read_text(encoding="ascii").splitlines():
                key, separator, value = line.partition(":")
                if separator and key in {"VmHWM", "VmRSS"}:
                    values[key] = int(value.strip().split()[0]) * 1024
            return values.get("VmHWM", values.get("VmRSS"))
        except (OSError, ValueError):
            return None
    if sys.platform == "darwin":
        try:
            value = subprocess.check_output(
                ["ps", "-o", "rss=", "-p", str(pid)], stderr=subprocess.DEVNULL
            )
            return int(value.strip()) * 1024
        except (OSError, ValueError, subprocess.CalledProcessError):
            return None
    return None


def read_windows_peak_rss(pid: int) -> int | None:
    try:
        from ctypes import wintypes

        class ProcessMemoryCounters(ctypes.Structure):
            _fields_ = [
                ("cb", wintypes.DWORD),
                ("PageFaultCount", wintypes.DWORD),
                ("PeakWorkingSetSize", ctypes.c_size_t),
                ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                ("PagefileUsage", ctypes.c_size_t),
                ("PeakPagefileUsage", ctypes.c_size_t),
            ]

        kernel32 = ctypes.windll.kernel32
        psapi = ctypes.windll.psapi
        handle = kernel32.OpenProcess(0x1000, False, pid)
        if not handle:
            return None
        try:
            counters = ProcessMemoryCounters()
            counters.cb = ctypes.sizeof(counters)
            if not psapi.GetProcessMemoryInfo(
                handle, ctypes.byref(counters), counters.cb
            ):
                return None
            return int(counters.PeakWorkingSetSize)
        finally:
            kernel32.CloseHandle(handle)
    except (AttributeError, OSError, TypeError):
        return None


def drop_filesystem_cache(command: str | None, cwd: Path) -> str:
    if command:
        result = subprocess.run(command, cwd=cwd, shell=True)
        if result.returncode != 0:
            raise BenchmarkError(
                f"cold-cache command failed with status {result.returncode}: {command}"
            )
        return f"custom: {command}"

    if sys.platform.startswith("linux"):
        subprocess.run(["sync"], check=True)
        try:
            Path("/proc/sys/vm/drop_caches").write_text("3\n", encoding="ascii")
        except OSError as error:
            raise BenchmarkError(
                "Linux cold-cache measurement needs permission to write "
                "/proc/sys/vm/drop_caches; pass --cold-command instead"
            ) from error
        return "Linux /proc/sys/vm/drop_caches"
    if sys.platform == "darwin":
        try:
            subprocess.run(["purge"], check=True)
        except (OSError, subprocess.CalledProcessError) as error:
            raise BenchmarkError(
                "macOS cold-cache measurement needs purge permission; "
                "pass --cold-command instead"
            ) from error
        return "macOS purge"
    raise BenchmarkError(
        "cold filesystem-cache measurement is not portable on this host; "
        "pass a privileged cache-drop command with --cold-command"
    )


def reset_cache(repository: Path) -> None:
    cache = repository / ".gitscry"
    if cache.exists():
        shutil.rmtree(cache)


def cache_metrics(repository: Path) -> dict[str, object]:
    cache_directory = repository / ".gitscry"
    total_bytes = 0
    if cache_directory.exists():
        total_bytes = sum(
            file.stat().st_size for file in cache_directory.rglob("*") if file.is_file()
        )
    objects: dict[str, int] = {}
    database = cache_directory / "cache.sqlite"
    if database.exists():
        try:
            uri = database.resolve().as_uri() + "?mode=ro"
            with sqlite3.connect(uri, uri=True) as connection:
                rows = connection.execute(
                    "SELECT name, SUM(pgsize) FROM dbstat GROUP BY name"
                )
                objects = {
                    str(name): int(size or 0)
                    for name, size in rows
                    if name is not None
                }
        except sqlite3.DatabaseError:
            # dbstat is a reporting aid, not part of GitScry's cache contract.
            objects = {}
    return {
        "total_bytes": total_bytes,
        "objects": dict(sorted(objects.items(), key=lambda item: (-item[1], item[0]))),
    }


def output_hash(result: ProcessResult) -> str:
    return hashlib.sha256(result.stdout + b"\0" + result.stderr).hexdigest()


def percentile(values: list[float], percentile_value: float) -> float:
    ordered = sorted(values)
    index = max(0, math.ceil(percentile_value * len(ordered)) - 1)
    return ordered[index]


def aggregate(samples: list[ProcessResult]) -> dict[str, object]:
    walls = [sample.wall_seconds for sample in samples]
    hashes = sorted({output_hash(sample) for sample in samples})
    return {
        "sample_count": len(samples),
        "wall_seconds": {
            "median": statistics_median(walls),
            "p95": percentile(walls, 0.95),
            "samples": walls,
        },
        "peak_rss_bytes": max(
            (sample.peak_rss_bytes or 0 for sample in samples), default=None
        ),
        "return_codes": sorted({sample.returncode for sample in samples}),
        "output_sha256": hashes,
        "output_stable": len(hashes) == 1,
    }


def statistics_median(values: list[float]) -> float:
    ordered = sorted(values)
    middle = len(ordered) // 2
    if len(ordered) % 2:
        return ordered[middle]
    return (ordered[middle - 1] + ordered[middle]) / 2


def run_scenario(
    binary: Path,
    repository: Path,
    command: CommandSpec,
    state: str,
    runs: int,
    timeout: float,
    cold_command: str | None,
    reset_before_run: bool,
) -> dict[str, object]:
    samples: list[ProcessResult] = []
    for iteration in range(runs + 1):
        if reset_before_run:
            reset_cache(repository)
        if state == "cold":
            drop_filesystem_cache(cold_command, repository)
        result = run_process([binary, *command.args], repository, timeout)
        if result.returncode != 0:
            raise BenchmarkError(
                f"{command.name} failed in {repository} with status {result.returncode}: "
                f"{result.stderr.decode(errors='replace')}"
            )
        if iteration > 0:
            samples.append(result)
    result = aggregate(samples)
    result["warmup_runs"] = 1
    result["state"] = state
    return result


def command_specs(probe_path: str) -> tuple[CommandSpec, ...]:
    return (
        CommandSpec("search", ("search", "fix")),
        CommandSpec("examples", ("examples", "fix", "--path", probe_path)),
        CommandSpec("failures", ("failures", "fix", "--path", probe_path)),
        CommandSpec("related", ("related", probe_path)),
        CommandSpec("tests", ("tests", probe_path)),
        CommandSpec("why", ("why", probe_path, "--line", "1")),
        CommandSpec(
            "regression",
            ("regression", "fix", "--path", probe_path),
        ),
        CommandSpec("trace-fix", ("trace-fix", "HEAD", "--path", probe_path)),
    )


def git_output(repository: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", *args], cwd=repository, check=True, capture_output=True, text=True
    )
    return result.stdout.strip()


def benchmark_repository(
    binary: Path,
    name: str,
    repository: Path,
    probe_path: str,
    runs: int,
    timeout: float,
    cold_command: str | None,
) -> dict[str, object]:
    if not (repository / ".git").exists():
        raise BenchmarkError(f"not a Git worktree: {repository}")
    probe = repository / probe_path
    if not probe.is_file():
        raise BenchmarkError(
            f"probe path does not exist in {name}: {probe_path}; "
            "pass --probe NAME=PATH for this checkout"
        )

    commands = command_specs(probe_path)
    index = CommandSpec("index", ("index",))
    index_states = {
        state: run_scenario(
            binary,
            repository,
            index,
            state,
            runs,
            timeout,
            cold_command,
            reset_before_run=True,
        )
        for state in ("cold", "warm")
    }
    measured_cache = cache_metrics(repository)

    command_states: dict[str, dict[str, object]] = {}
    for command in commands:
        command_states[command.name] = {
            state: run_scenario(
                binary,
                repository,
                command,
                state,
                runs,
                timeout,
                cold_command,
                reset_before_run=False,
            )
            for state in ("cold", "warm")
        }

    return {
        "name": name,
        "path": str(repository),
        "probe_path": probe_path,
        "head": git_output(repository, "rev-parse", "HEAD"),
        "commit_count": int(git_output(repository, "rev-list", "--count", "HEAD")),
        "git_version": git_output(repository, "--version"),
        "cache_after_index": measured_cache,
        "index": index_states,
        "commands": command_states,
    }


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(
        description="Measure reproducible GitScry release cache and CLI baselines."
    )
    result.add_argument(
        "--binary",
        type=Path,
        default=Path("target/release/gitscry"),
        help="release binary (default: target/release/gitscry)",
    )
    result.add_argument(
        "--repo",
        action="append",
        required=True,
        metavar="NAME=PATH",
        help="validation checkout; provide medium=... and large=...",
    )
    result.add_argument(
        "--probe",
        action="append",
        default=[],
        metavar="NAME=PATH",
        help="override the command probe path for a repository",
    )
    result.add_argument("--runs", type=int, default=MIN_RUNS)
    result.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT_SECONDS)
    result.add_argument(
        "--cold-command",
        help="privileged command that drops filesystem cache before cold samples",
    )
    result.add_argument(
        "--output",
        type=Path,
        help="also write the JSON report to this path",
    )
    return result


def main() -> None:
    arguments = parser().parse_args()
    if arguments.runs < MIN_RUNS:
        raise BenchmarkError(f"--runs must be at least {MIN_RUNS}")
    if arguments.timeout <= 0:
        raise BenchmarkError("--timeout must be positive")
    if not arguments.binary.is_file():
        raise BenchmarkError(f"release binary does not exist: {arguments.binary}")

    repositories = dict(parse_assignment(value, "--repo") for value in arguments.repo)
    missing = {"medium", "large"} - repositories.keys()
    if missing:
        raise BenchmarkError(
            "--repo must include " + ", ".join(sorted(missing))
        )
    probes = dict(parse_assignment(value, "--probe") for value in arguments.probe)
    probe_paths = {**DEFAULT_PROBES, **probes}

    cold_method = (
        f"custom: {arguments.cold_command}"
        if arguments.cold_command
        else "platform default (requires privileged cache drop)"
    )
    report = {
        "schema": 1,
        "reference_baseline": REFERENCE_BASELINE,
        "environment": {
            "timestamp_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "platform": platform.platform(),
            "python": sys.version,
            "machine": platform.machine(),
            "processor": platform.processor(),
            "git_version": subprocess.run(
                ["git", "--version"], check=True, capture_output=True, text=True
            ).stdout.strip(),
            "binary": str(arguments.binary.resolve()),
            "runs": arguments.runs,
            "timeout_seconds": arguments.timeout,
            "cold_cache_method": cold_method,
        },
        "reproduction": " ".join(
            subprocess.list2cmdline([sys.executable, *sys.argv]).split()
        ),
        "repositories": [],
    }

    for name, raw_path in sorted(repositories.items()):
        repository = Path(raw_path).resolve()
        report["repositories"].append(
            benchmark_repository(
                arguments.binary.resolve(),
                name,
                repository,
                probe_paths.get(name, ""),
                arguments.runs,
                arguments.timeout,
                arguments.cold_command,
            )
        )

    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
    print(rendered, end="")
    if arguments.output:
        arguments.output.parent.mkdir(parents=True, exist_ok=True)
        arguments.output.write_text(rendered, encoding="utf-8")


if __name__ == "__main__":
    try:
        main()
    except (BenchmarkError, OSError, subprocess.CalledProcessError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(2)
