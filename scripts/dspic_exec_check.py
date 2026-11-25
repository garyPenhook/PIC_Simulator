#!/usr/bin/env python3
"""Builds the dsPIC33CK test programs of the Basil compiler and checks their `expect:` results in picsim's core.

Usage: dspic_exec_check.py [--basil PATH] [--dir DIR] [names...]
Each program names its expectations in comments near the top (`' expect: VAR=value ...`, as scripts/exec-test33.py of the
compiler reads them) and must reach `__halt`.
"""
import argparse, glob, os, re, subprocess, sys, tempfile

ap = argparse.ArgumentParser()
ap.add_argument("--basil", default=os.path.expanduser("~/apps/Basic_Compiler/target/release/basil"))
ap.add_argument("--dir", default=os.path.expanduser("~/apps/Basic_Compiler/tests/exec_dspic"))
ap.add_argument("--keep", action="store_true")
ap.add_argument("names", nargs="*")
a = ap.parse_args()
root = os.path.dirname(os.path.abspath(__file__)) + "/.."
subprocess.run(["cargo", "build", "-q", "-p", "picsim", "--example", "dspic_run"], cwd=root, check=True)
run = root + "/target/debug/examples/dspic_run"
dev = root + "/devices/DSPIC33CK64MC105.toml"
files = [os.path.join(a.dir, n + ".bas") for n in a.names] if a.names else sorted(glob.glob(a.dir + "/*.bas"))
work = tempfile.mkdtemp(prefix="dsx-")
fails = 0
for src in files:
    name = os.path.splitext(os.path.basename(src))[0]
    text = open(src).read()
    expect = {}
    for line in re.findall(r"^'\s*expect:(.*)$", text, re.M):
        for kv in line.split():
            k, v = kv.split("=")
            expect[k] = int(v, 0)
    hexf = os.path.join(work, name + ".hex")
    r = subprocess.run([a.basil, "--keep-vars", src, "-o", hexf], capture_output=True, text=True)
    if r.returncode != 0:
        print(f"SKIP {name}: compile failed: {r.stderr.strip().splitlines()[-1] if r.stderr.strip() else ''}")
        continue
    asm = open(os.path.join(work, name + ".S")).read()
    vars_ = {m.group(1): (int(m.group(2), 16), int(m.group(3))) for m in re.finditer(r"^\s*\.equ (\w+), 0x([0-9A-F]+)\s*; .*\((\d+) bytes\)", asm, re.M)}
    halt = re.search(r"^\s+0x([0-9a-f]+)\s+__halt\s*$", open(os.path.join(work, name + ".map")).read(), re.M)
    if not halt:
        print(f"FAIL {name}: no __halt")
        fails += 1
        continue
    dumps = []
    for k in expect:
        if k in vars_:
            dumps += ["--dump", f"{vars_[k][0]:X}:{vars_[k][1]}"]
    stim = root + "/tests/dspic/" + name + ".stim.toml"
    extra = ["--stim", stim] if os.path.exists(stim) else []
    p = subprocess.run([run, hexf, dev, "--until", halt.group(1), "--max-cycles", "200000000"] + extra + dumps, capture_output=True, text=True)
    out = p.stdout
    pc = re.search(r"^PC ([0-9A-F]+)", out, re.M)
    bad = []
    if not pc or int(pc.group(1), 16) != int(halt.group(1), 16):
        bad.append(f"did not reach __halt (PC {pc.group(1) if pc else '?'}, __halt {halt.group(1)})")
    blocks = re.findall(r"^DUMP [0-9A-F]+ ([0-9a-f ]+)$", out, re.M)
    bi = 0
    for k, want in expect.items():
        if k not in vars_:
            bad.append(f"{k}: no such symbol")
            continue
        size = vars_[k][1]
        got = sum(int(b, 16) << (8 * i) for i, b in enumerate(blocks[bi].split()[:size]))
        bi += 1
        m = (1 << (8 * size)) - 1
        if got != want & m:
            bad.append(f"{k}: want {want & m} got {got}")
    cyc = re.search(r"^'\s*cycles:\s*(\d+)\s+(\d+)", text, re.M)
    if cyc:
        c = re.search(r"^CYCLES (\d+)", out, re.M)
        if c and not (int(cyc.group(1)) <= int(c.group(1)) <= int(cyc.group(2))):
            bad.append(f"cycles {c.group(1)} outside {cyc.group(1)}..{cyc.group(2)}")
    if bad:
        fails += 1
        print(f"FAIL {name}: " + "; ".join(bad) + (" [" + p.stderr.strip().replace("\n", ", ") + "]" if p.stderr.strip() else ""))
    else:
        print(f"ok   {name}" + (" [" + p.stderr.strip().replace("\n", ", ") + "]" if p.stderr.strip() else ""))
sys.exit(1 if fails else 0)
