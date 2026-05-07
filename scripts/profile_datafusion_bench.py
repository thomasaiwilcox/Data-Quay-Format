#!/usr/bin/env python3
"""Record a DataFusion/COVE benchmark profile into an Instruments .trace bundle.

This script is intended for the `crates/cove-datafusion/benches/m6.rs` Criterion
benchmarks, especially the `parquet-compare` tracks. It builds the bench binary
with DWARF symbols, launches a single Criterion benchmark in `--profile-time`
mode under `xctrace`, and writes a `.trace` bundle plus the target stdout log.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import time
from typing import Iterable


REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUTPUT_DIR = REPO_ROOT / "artifacts" / "instruments"
DEFAULT_TEMPLATE = "Time Profiler"
DEFAULT_PROFILE_SECONDS = 15
DEFAULT_PACKAGE = "cove-datafusion"
DEFAULT_FEATURES = "parquet-compare"
DEFAULT_BENCH = "m6"
DEFAULT_RUNNER = "criterion"
DEFAULT_STAGE = "execute-only"
DEFAULT_ATTACH_STARTUP_DELAY_MS = 750
DEFAULT_RUN_SECONDS_MARGIN = 2

TRACKS = {
    "tiny-full-scan": "parquet_compare_full_scan",
    "tiny-projection-scan": "parquet_compare_projection_scan",
    "tiny-low-cardinality-filter": "parquet_compare_low_cardinality_filter",
    "tiny-numeric-range-filter": "parquet_compare_numeric_range_filter",
    "tiny-wide-projection-filter": "parquet_compare_wide_projection_filter",
    "scan-heavy-full-scan": "parquet_compare_scan_heavy_full_scan",
    "scan-heavy-projection-scan": "parquet_compare_scan_heavy_projection_scan",
    "scan-heavy-low-cardinality-filter": "parquet_compare_scan_heavy_low_cardinality_filter",
    "scan-heavy-numeric-range-filter": "parquet_compare_scan_heavy_numeric_range_filter",
    "scan-heavy-wide-projection-filter": "parquet_compare_scan_heavy_wide_projection_filter",
    "cold-context-full-scan": "parquet_compare_cold_context_full_scan",
    "cold-context-numeric-range-filter": "parquet_compare_cold_context_numeric_range_filter",
}

ENGINES = ("cove", "parquet")
RUNNERS = ("criterion", "attached-query")
PROFILE_STAGES = ("full-query", "planning-only", "execute-only")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Record one cove-datafusion benchmark or dedicated query profile "
            "under Instruments Time Profiler and emit a .trace bundle."
        )
    )
    parser.add_argument(
        "--runner",
        choices=RUNNERS,
        default=DEFAULT_RUNNER,
        help=(
            "Profiling workflow. 'criterion' records the existing Criterion benchmark; "
            "'attached-query' launches the dedicated query profiler, waits for setup, "
            "then attaches xctrace to the hot loop."
        ),
    )
    parser.add_argument(
        "--track",
        choices=sorted(TRACKS),
        default="scan-heavy-full-scan",
        help="Friendly benchmark track name.",
    )
    parser.add_argument(
        "--engine",
        choices=ENGINES,
        default="cove",
        help="Which side of the compare pair to profile.",
    )
    parser.add_argument(
        "--benchmark-id",
        help=(
            "Override the Criterion benchmark id directly, for example "
            "'parquet_compare_scan_heavy_full_scan/cove'."
        ),
    )
    parser.add_argument(
        "--profile-seconds",
        type=int,
        default=DEFAULT_PROFILE_SECONDS,
        help="Recording duration in seconds.",
    )
    parser.add_argument(
        "--stage",
        choices=PROFILE_STAGES,
        default=DEFAULT_STAGE,
        help=(
            "For --runner attached-query: which phase to loop under the profiler. "
            "'full-query' includes SQL/planning/execution after setup, "
            "'planning-only' repeats plan creation only, and "
            "'execute-only' repeats just physical-plan execution."
        ),
    )
    parser.add_argument(
        "--run-seconds",
        type=int,
        help=(
            "For --runner attached-query: how long the target should loop after "
            "profiling starts. Defaults to profile-seconds + 2."
        ),
    )
    parser.add_argument(
        "--startup-delay-ms",
        type=int,
        default=DEFAULT_ATTACH_STARTUP_DELAY_MS,
        help=(
            "For --runner attached-query: extra delay after xctrace reports that "
            "recording has started, before releasing the target into the hot loop."
        ),
    )
    parser.add_argument(
        "--template",
        default=DEFAULT_TEMPLATE,
        help="Instruments template name to use with xctrace.",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=DEFAULT_OUTPUT_DIR,
        help="Directory to store the .trace bundle and stdout log.",
    )
    parser.add_argument(
        "--name",
        help="Optional base name for the output trace bundle.",
    )
    parser.add_argument(
        "--bench",
        default=DEFAULT_BENCH,
        help="Cargo bench target name. Defaults to m6.",
    )
    parser.add_argument(
        "--package",
        default=DEFAULT_PACKAGE,
        help="Cargo package containing the bench target.",
    )
    parser.add_argument(
        "--features",
        default=DEFAULT_FEATURES,
        help="Cargo features to enable while building the bench binary.",
    )
    parser.add_argument(
        "--skip-build",
        action="store_true",
        help="Skip cargo build and use the newest matching bench executable.",
    )
    parser.add_argument(
        "--binary",
        type=Path,
        help="Use an explicit bench executable path instead of resolving one.",
    )
    parser.add_argument(
        "--criterion-arg",
        action="append",
        default=[],
        help="Additional raw Criterion argument to append to the launched bench process.",
    )
    parser.add_argument(
        "--xctrace-arg",
        action="append",
        default=[],
        help="Additional raw xctrace argument to append before --launch.",
    )
    parser.add_argument(
        "--list-tracks",
        action="store_true",
        help="Print available friendly track names and exit.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print the resolved commands and exit without running xctrace.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.list_tracks:
        for name, criterion_name in TRACKS.items():
            print(f"{name:35s} -> {criterion_name}/<cove|parquet>")
        return 0

    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)

    bench_executable = resolve_bench_executable(args)
    timestamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    if args.runner == "criterion":
        benchmark_id = args.benchmark_id or f"{TRACKS[args.track]}/{args.engine}"
        default_name = f"{timestamp}-{sanitize_name(benchmark_id)}"
    else:
        benchmark_id = None
        default_name = (
            f"{timestamp}-profile-query-"
            f"{sanitize_name(args.track)}-"
            f"{sanitize_name(args.engine)}-"
            f"{sanitize_name(args.stage)}"
        )
    base_name = args.name or default_name
    trace_path = output_dir / f"{base_name}.trace"
    stdout_path = output_dir / f"{base_name}.stdout.log"

    print(f"Bench executable: {bench_executable}")
    print(f"Trace bundle:    {trace_path}")
    print(f"Stdout log:      {stdout_path}")
    if args.runner == "criterion":
        xctrace_cmd = build_launch_xctrace_command(
            template=args.template,
            trace_path=trace_path,
            stdout_path=stdout_path,
            bench_executable=bench_executable,
            benchmark_id=benchmark_id,
            profile_seconds=args.profile_seconds,
            criterion_args=args.criterion_arg,
            extra_xctrace_args=args.xctrace_arg,
        )
        print(f"Criterion id:    {benchmark_id}")
        print("xctrace command:")
        print("  " + " ".join(shlex.quote(part) for part in xctrace_cmd))

        if args.dry_run:
            return 0

        result = subprocess.run(xctrace_cmd, cwd=REPO_ROOT)
        if result.returncode != 0:
            return result.returncode
    else:
        if args.benchmark_id:
            raise SystemExit("--benchmark-id is only valid with --runner criterion")
        if args.track.startswith("cold-context-"):
            raise SystemExit(
                "cold-context tracks are intentionally setup-heavy and are not supported "
                "with --runner attached-query"
            )

        run_seconds = args.run_seconds or (args.profile_seconds + DEFAULT_RUN_SECONDS_MARGIN)
        target_cmd = build_profile_query_command(
            bench_executable=bench_executable,
            track=args.track,
            engine=args.engine,
            stage=args.stage,
            run_seconds=run_seconds,
        )
        print("Profile target command:")
        print("  " + " ".join(shlex.quote(part) for part in target_cmd))
        if args.dry_run:
            attach_preview = build_attach_xctrace_command(
                template=args.template,
                trace_path=trace_path,
                pid=12345,
                profile_seconds=args.profile_seconds,
                extra_xctrace_args=args.xctrace_arg,
            )
            print("xctrace attach command:")
            print("  " + " ".join(shlex.quote(part) for part in attach_preview))
            return 0

        result = run_attached_query_profile(
            trace_path=trace_path,
            stdout_path=stdout_path,
            target_cmd=target_cmd,
            template=args.template,
            profile_seconds=args.profile_seconds,
            startup_delay_ms=args.startup_delay_ms,
            extra_xctrace_args=args.xctrace_arg,
            run_seconds=run_seconds,
        )
        if result != 0:
            return result

    print()
    print("Recording complete.")
    print(f"Open in Instruments: open {shlex.quote(str(trace_path))}")
    return 0


def resolve_bench_executable(args: argparse.Namespace) -> Path:
    if args.binary:
        candidate = args.binary.resolve()
        if not candidate.is_file():
            raise SystemExit(f"bench executable does not exist: {candidate}")
        return candidate

    if not args.skip_build:
        executable = cargo_build_bench(args.package, args.bench, args.features)
        if executable is not None:
            return executable

    candidate = newest_bench_executable(args.bench)
    if candidate is None:
        raise SystemExit(
            "could not resolve bench executable; run without --skip-build or pass --binary"
        )
    return candidate


def cargo_build_bench(package: str, bench: str, features: str) -> Path | None:
    env = os.environ.copy()
    env.setdefault("CARGO_PROFILE_BENCH_DEBUG", "true")
    env.setdefault("CARGO_PROFILE_BENCH_SPLIT_DEBUGINFO", "packed")

    cmd = [
        "cargo",
        "bench",
        "-p",
        package,
        "--features",
        features,
        "--bench",
        bench,
        "--no-run",
        "--message-format=json",
    ]
    print("Building bench binary:")
    print("  " + " ".join(shlex.quote(part) for part in cmd))
    proc = subprocess.run(
        cmd,
        cwd=REPO_ROOT,
        env=env,
        text=True,
        capture_output=True,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stdout)
        sys.stderr.write(proc.stderr)
        raise SystemExit(proc.returncode)

    executable: Path | None = None
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if payload.get("reason") != "compiler-artifact":
            continue
        target = payload.get("target", {})
        if target.get("name") != bench:
            continue
        if "bench" not in target.get("kind", []):
            continue
        candidate = payload.get("executable")
        if candidate:
            executable = Path(candidate)
    if executable is None:
        executable = newest_bench_executable(bench)
    if executable is None:
        raise SystemExit("cargo build succeeded but no bench executable was resolved")
    return executable


def newest_bench_executable(bench: str) -> Path | None:
    bench_dir = REPO_ROOT / "target" / "release" / "deps"
    candidates = [
        path
        for path in bench_dir.glob(f"{bench}-*")
        if path.is_file() and os.access(path, os.X_OK)
    ]
    if not candidates:
        return None
    return max(candidates, key=lambda path: path.stat().st_mtime)


def build_launch_xctrace_command(
    *,
    template: str,
    trace_path: Path,
    stdout_path: Path,
    bench_executable: Path,
    benchmark_id: str,
    profile_seconds: int,
    criterion_args: Iterable[str],
    extra_xctrace_args: Iterable[str],
) -> list[str]:
    cmd = [
        "xcrun",
        "xctrace",
        "record",
        "--template",
        template,
        "--output",
        str(trace_path),
        "--target-stdout",
        str(stdout_path),
        "--no-prompt",
    ]
    cmd.extend(extra_xctrace_args)
    cmd.extend(
        [
            "--launch",
            "--",
            str(bench_executable),
            benchmark_id,
            "--profile-time",
            str(profile_seconds),
        ]
    )
    cmd.extend(criterion_args)
    return cmd


def build_profile_query_command(
    *,
    bench_executable: Path,
    track: str,
    engine: str,
    stage: str,
    run_seconds: int,
) -> list[str]:
    return [
        str(bench_executable),
        "profile-query",
        "--track",
        track,
        "--engine",
        engine,
        "--stage",
        stage,
        "--run-seconds",
        str(run_seconds),
    ]


def build_attach_xctrace_command(
    *,
    template: str,
    trace_path: Path,
    pid: int,
    profile_seconds: int,
    extra_xctrace_args: Iterable[str],
) -> list[str]:
    cmd = [
        "xcrun",
        "xctrace",
        "record",
        "--template",
        template,
        "--output",
        str(trace_path),
        "--no-prompt",
    ]
    cmd.extend(extra_xctrace_args)
    cmd.extend(["--attach", str(pid), "--time-limit", f"{profile_seconds}s"])
    return cmd


def run_attached_query_profile(
    *,
    trace_path: Path,
    stdout_path: Path,
    target_cmd: list[str],
    template: str,
    profile_seconds: int,
    startup_delay_ms: int,
    extra_xctrace_args: Iterable[str],
    run_seconds: int,
) -> int:
    with stdout_path.open("w", encoding="utf-8") as log_file:
        target = subprocess.Popen(
            target_cmd,
            cwd=REPO_ROOT,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        try:
            ready_line = wait_for_profile_ready(target, log_file)
            pid = parse_profile_ready_pid(ready_line)
            attach_cmd = build_attach_xctrace_command(
                template=template,
                trace_path=trace_path,
                pid=pid,
                profile_seconds=profile_seconds,
                extra_xctrace_args=extra_xctrace_args,
            )
            print("xctrace attach command:")
            print("  " + " ".join(shlex.quote(part) for part in attach_cmd))

            attach = subprocess.Popen(
                attach_cmd,
                cwd=REPO_ROOT,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
            )
            wait_for_xctrace_start(attach, log_file)
            if startup_delay_ms > 0:
                time.sleep(startup_delay_ms / 1000.0)

            assert target.stdin is not None
            target.stdin.write("go\n")
            target.stdin.flush()
            target.stdin.close()

            attach_rc = attach.wait()
            drain_process_output(attach.stdout, log_file)

            drain_process_output(target.stdout, log_file)

            try:
                target.wait(timeout=run_seconds + 10)
            except subprocess.TimeoutExpired:
                terminate_process(target)
                raise SystemExit(
                    "profile target did not exit after tracing completed; it was terminated"
                )

            if attach_rc != 0:
                return attach_rc
            return target.returncode or 0
        finally:
            terminate_process(target, quiet=True)


def wait_for_profile_ready(target: subprocess.Popen[str], log_file) -> str:
    assert target.stdout is not None
    while True:
        line = target.stdout.readline()
        if line:
            log_file.write(line)
            log_file.flush()
            if line.startswith("PROFILE_READY "):
                return line.strip()
        elif target.poll() is not None:
            raise SystemExit(
                f"profile target exited before signaling readiness (exit code {target.returncode})"
            )


def wait_for_xctrace_start(attach: subprocess.Popen[str], log_file) -> None:
    assert attach.stdout is not None
    while True:
        line = attach.stdout.readline()
        if line:
            log_file.write(line)
            log_file.flush()
            if line.startswith("Starting recording ") or "Attaching to:" in line:
                return
        elif attach.poll() is not None:
            drain_process_output(attach.stdout, log_file)
            raise SystemExit(
                f"xctrace exited before recording started (exit code {attach.returncode})"
            )


def parse_profile_ready_pid(line: str) -> int:
    for token in line.split():
        if token.startswith("pid="):
            return int(token.split("=", 1)[1])
    raise SystemExit(f"could not parse pid from readiness line: {line}")


def drain_process_output(stream, log_file) -> None:
    if stream is None:
        return
    remaining = stream.read()
    if remaining:
        log_file.write(remaining)
        log_file.flush()


def terminate_process(target: subprocess.Popen[str], quiet: bool = False) -> None:
    if target.poll() is not None:
        return
    target.terminate()
    try:
        target.wait(timeout=5)
    except subprocess.TimeoutExpired:
        target.kill()
        target.wait(timeout=5)
    if not quiet:
        print(f"terminated lingering profile target (pid {target.pid})")


def sanitize_name(name: str) -> str:
    return "".join(ch if ch.isalnum() or ch in ("-", "_") else "-" for ch in name)


if __name__ == "__main__":
    raise SystemExit(main())
