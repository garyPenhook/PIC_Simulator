#!/usr/bin/env python3
"""Run an MDB probe (tests/probe/*.S) on basil-sim and on MDB and compare them.

    tools/probe.py OUTDIR SRC PART 'K=V K=V' ['K=V ...' ...]

Each quoted argument is one variant: its K=V pairs become -D defines for pic-as. Every variant is assembled, run
to the label `done` on basil-sim (target/release/basil-sim, build it first) and, in one batched session, on MDB;
the cycle count at `done` and the 8 bytes at 20h are printed for both, with `ok` or `DIFF`.

    tools/probe.py /tmp/p tests/probe/t0_poll.S 16F17146 'PERIOD=9 PRE=7 CLR=1' 'PERIOD=9 PRE=8 CLR=1'

MDB waits at most 20 s per variant; one that does not reach `done` by then shows a stopwatch count far beyond
basil-sim's and `?` bytes. Override the tools with PICAS, DFP (the pack's xc8 folder) or MDB in the environment.
"""
import glob, os, re, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def newest(pattern):
    found = sorted(glob.glob(os.path.expanduser(pattern)))
    return found[-1] if found else None


if len(sys.argv) < 5:
    sys.exit(__doc__)
out, src, part = sys.argv[1], os.path.abspath(sys.argv[2]), sys.argv[3]
variants = sys.argv[4:]
PICAS = os.environ.get("PICAS") or newest("/opt/microchip/xc8/v*/pic-as/bin/pic-as")
DFP = os.environ.get("DFP") or newest("~/.mchp_packs/Microchip/PIC16F1xxxx_DFP/*/xc8")
MDB = os.environ.get("MDB") or newest("/opt/microchip/mplabx/v*/mplab_platform/bin/mdb.sh")
SIM = os.path.join(ROOT, "target/release/basil-sim")
for name, path in [("pic-as", PICAS), ("PIC16F1xxxx_DFP", DFP), ("MDB", MDB), ("basil-sim", SIM)]:
    if not path or not os.path.exists(path):
        sys.exit(f"{name} not found")
os.makedirs(out, exist_ok=True)

jobs = []
for i, v in enumerate(variants):
    d = os.path.abspath(os.path.join(out, f"v{i}"))
    os.makedirs(d, exist_ok=True)
    defs = [f"-D{kv}" for kv in v.split()]
    r = subprocess.run([PICAS, f"-mcpu={part}", f"-mdfp={DFP}", *defs, "-Wl,-presetVec=0h", "-Wa,-a",
                        "-o", os.path.join(d, "p.hex"), src], cwd=d, capture_output=True, text=True)
    if r.returncode:
        sys.exit(f"{v}: {r.stdout}{r.stderr}")
    lst = open(glob.glob(os.path.join(d, "*.lst"))[0]).read()
    done = int(re.search(r"\bdone ([0-9A-F]{4})\b", lst).group(1), 16)  # word address, from the symbol table
    open(os.path.join(d, "p.S"), "w").write(f";SYM done 0x{done:X}\n;SYM buf 0x20\n")
    s = subprocess.run([SIM, os.path.join(d, "p.hex"), "--device", part, "--asm", os.path.join(d, "p.S"),
                        "--until", "done", "--dump", "buf:8", "--max-cycles", "100000"], capture_output=True, text=True).stdout
    sc = re.search(r"cycles: (\d+)", s).group(1)
    sb = re.search(r"buf = \d+  \(([0-9A-F ]+)\)", s).group(1)
    jobs.append((v, os.path.join(d, "p.hex"), done, sc, sb))

script = [f"device PIC{part}", "hwtool SIM"]
for i, (v, hexf, done, _, _) in enumerate(jobs):
    script += [f"program {hexf}", f"break *0x{done:X}", "run", "wait 20000", "stopwatch", "x /8xb 0x20", f"echo END_{i}", "delete"]
script.append("quit")
sp = os.path.join(out, "run.mdb")
open(sp, "w").write("\n".join(script) + "\n")
mo = subprocess.run([MDB, sp], capture_output=True, text=True, timeout=1800).stdout
open(os.path.join(out, "mdb.log"), "w").write(mo)

segs = re.split(r"END_\d+", mo)
bad = 0
for i, (v, _, _, sc, sb) in enumerate(jobs):
    seg = segs[i] if i < len(segs) else ""
    mc = (re.findall(r"Stopwatch cycle count = (\d+)", seg) or ["?"])[-1]
    m = re.search(r"x /8xb 0x20\s*\n((?:[0-9a-fA-F]{2}\s+){8})", seg)
    mb = " ".join(x.upper() for x in m.group(1).split()) if m else "?"
    ok = sc == mc and sb == mb
    bad += not ok
    print(f"{'ok  ' if ok else 'DIFF'} {v:<34} sim {sc:>5} [{sb}]  mdb {mc:>5} [{mb}]")
print(f"{len(jobs) - bad}/{len(jobs)} agree")
sys.exit(1 if bad else 0)
