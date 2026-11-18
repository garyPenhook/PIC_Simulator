#!/usr/bin/env python3
"""Run compiler execution suites with retained logs and a machine-readable summary.

Requires the sibling Basic_Compiler checkout and release binaries. DMA replay and
explicit MDB-only programs are excluded from sim runs, rather than silently run
in another engine. Use the Rust DMA tests for native engine coverage.
"""
import argparse
import json
import os
import signal
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
MATRIX = {
    "18F27Q10": "exec",
    "18F16Q41": "exec_q41",
    "18F25K22": "exec_k22",
    "18F45K22": "exec_k22",
    "16F17146": "exec_p16",
    "16F13145": "exec_p13",
    "16F13245": "exec_p132",
    "16F15244": "exec_p152",
    "16F18056": "exec_p180",
    "16F18346": "exec_p183",
}


# Reviewed test assumptions, not simulator failures. Each exclusion remains in the
# JSON report. Native DMA movement is tested in Rust and tests/sim/dma_q41.bas.
INCOMPATIBLE = {
    ("18F16Q41", "lib_dma.bas"): "expects MDB to leave DGO permanently set; native DMA completes",
    ("18F16Q41", "lib_dma_eeprom.bas"): "expects MDB's absent DMA engine (DGO/XIP never advance)",
    ("18F16Q41", "lib_dma_flash.bas"): "expects MDB's absent DMA engine (DGO/XIP never advance)",
    ("18F45K22", "lib_spi.bas"): "fixture hardcodes 28-pin MSSP2 pins; 40-pin MSSP2 is on PORTD",
    ("18F45K22", "lib_pwm_out.bas"): "fixture hardcodes 28-pin CCP4 RB0; 40-pin CCP4 is RD1",
}


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--compiler-root", type=Path, default=ROOT.parent / "Basic_Compiler")
    ap.add_argument("--output", type=Path, default=ROOT / "target/regression")
    ap.add_argument("--engine", choices=("sim", "both"), default="sim")
    ap.add_argument("--device", action="append", choices=MATRIX)
    optimization = ap.add_mutually_exclusive_group()
    optimization.add_argument("--optimized-only", action="store_true")
    optimization.add_argument("--unoptimized-only", action="store_true")
    ap.add_argument("--timeout", type=int, default=1800)
    args = ap.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    compiler = args.compiler_root.resolve()
    results = []
    for part in args.device or MATRIX:
        files = sorted((compiler / "tests" / MATRIX[part]).glob("*.bas"))
        excluded, chosen = [], []
        for file in files:
            text = file.read_text()
            reason = INCOMPATIBLE.get((part, file.name)) if args.engine == "sim" else None
            if reason:
                pass
            elif re.search(r"^'\s*dma:", text, re.M):
                reason = "requires legacy MDB DMA replay"
            elif args.engine == "sim" and re.search(r"^'\s*engine:\s*mdb\b", text, re.M):
                reason = "explicit MDB-only program"
            elif args.engine == "both" and re.search(r"^'\s*(engine:|stim:)", text, re.M):
                reason = "not a two-engine comparison"
            (excluded if reason else chosen).append({"file": file.name, "reason": reason} if reason else file)
        for flags in ([""] if args.optimized_only else ["-O0"] if args.unoptimized_only else ["", "-O0"]):
            label = f"{part}-{'O0' if flags else 'O'}-{args.engine}"
            cmd = [sys.executable, str(compiler / "scripts/exec-test.py"),
                   "--basil", str(compiler / "target/release/basil"),
                   "--sim", str(ROOT / "target/release/basil-sim"),
                   "--device", part, "--engine", args.engine, f"--flags={flags}",
                   *map(str, chosen)]
            print(f"Running {label}: {len(chosen)} programs, {len(excluded)} excluded", flush=True)
            if not chosen:
                raise SystemExit(f"no programs selected for {part}")
            run = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
            try:
                stdout, stderr = run.communicate(timeout=args.timeout)
                output, code = stdout + stderr, run.returncode
            except subprocess.TimeoutExpired:
                os.killpg(run.pid, signal.SIGTERM)
                try:
                    stdout, stderr = run.communicate(timeout=10)
                except subprocess.TimeoutExpired:
                    os.killpg(run.pid, signal.SIGKILL)
                    stdout, stderr = run.communicate()
                output, code = stdout + stderr + "\nTIMEOUT\n", 124
            (args.output / f"{label}.log").write_text(output)
            row = {"device": part, "flags": flags, "engine": args.engine, "exit_code": code,
                   "passed": len(re.findall(r"^ok\s", output, re.M)),
                   "failures": re.findall(r"^FAIL.*$", output, re.M),
                   "skipped": re.findall(r"^skip.*$", output, re.M), "excluded": excluded}
            results.append(row)
            (args.output / "summary.json").write_text(json.dumps(results, indent=2) + "\n")
            print(f"  {row['passed']} passed, {len(row['failures'])} failed, exit {code}", flush=True)
    return int(any(row["exit_code"] for row in results))


if __name__ == "__main__":
    sys.exit(main())
