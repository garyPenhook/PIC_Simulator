#!/usr/bin/env python3
"""Differential test of picsim's dsPIC33 core against MPLAB X's MDB simulator.

Random instructions are taken from the decoder (`examples/dspic_dis.rs`), kept when the XC-DSC assembler accepts them, and run
one at a time between two breakpoints in a program that sets up every register from a random state first. After each
instruction the program saves W0..W15, SR, the accumulators, CORCON and checksums of the RAM it may touch; MDB's stopwatch
gives the cycles of the instruction. picsim runs the same program; any difference is printed with the instruction.

Usage: dspic_diff.py [--tests N] [--seed S] [--per-run K] [--only mnemonic,mnemonic] [--keep]
"""
import argparse, os, random, re, subprocess, sys, tempfile

ap = argparse.ArgumentParser()
ap.add_argument("--tests", type=int, default=200)
ap.add_argument("--seed", type=int, default=1)
ap.add_argument("--per-run", type=int, default=50)
ap.add_argument("--only", default="")
ap.add_argument("--skip", default="")
ap.add_argument("--keep", action="store_true")
ap.add_argument("--no-cycles", action="store_true")
ap.add_argument("--xc", default="/opt/microchip/xc-dsc/v4.00/bin/xc-dsc-gcc")
ap.add_argument("--dfp", default=os.path.expanduser("~/.mchp_packs/Microchip/dsPIC33CK-MC_DFP/1.11.412/xc16"))
ap.add_argument("--mdb", default="/opt/microchip/mplabx/v6.35/mplab_platform/bin/mdb.sh")
ap.add_argument("--texts", default="", help="a file with one instruction text per line (statements separated by ;), each tested once")
ap.add_argument("--text", default="", help="test this one instruction text instead of random ones (repeated --tests times with random state)")
a = ap.parse_args()
root = os.path.dirname(os.path.abspath(__file__)) + "/.."
rnd = random.Random(a.seed)
subprocess.run(["cargo", "build", "-q", "-p", "picsim", "--example", "dspic_dis", "--example", "dspic_run"], cwd=root, check=True)
DIS, RUN = root + "/target/debug/examples/dspic_dis", root + "/target/debug/examples/dspic_run"
DEV = root + "/devices/DSPIC33CK64MC105.toml"
EXCLUDE = re.compile(r"^(bra|call|goto|rcall|return|retfie|retlw|do|repeat|pwrsav|reset|clrwdt|disi|lnk|ulnk|ctxtswp|bootswp|push\.s|pop\.s|movpag|cpb(gt|lt|ne|eq)|nopr|div|divf|div2)")
if a.skip:
    EXCLUDE = re.compile(EXCLUDE.pattern[:-1] + "|" + "|".join(a.skip.split(",")) + ")")
ONLY = re.compile("^(" + "|".join(a.only.split(",")) + ")(\\.|\\s|$)") if a.only else None

def gen_texts(n):
    out = []
    while len(out) < n:
        batch = []
        for _ in range(4000):
            t = rnd.randrange(256)
            batch.append((t << 16 | rnd.getrandbits(16), rnd.getrandbits(16)))
        dis = subprocess.run([DIS], input="\n".join(f"{w:06X} {w2:06X}" for w, w2 in batch), capture_output=True, text=True).stdout.splitlines()
        cand = []
        for l in dis:
            k, _, text = l.split("\t")
            if k == "0" or EXCLUDE.match(text):
                continue
            if ONLY and not ONLY.match(text):
                continue
            if re.match(r"\S+\.b\s", text) and re.search(r"\[(?:\+\+|--)?w15", text):
                continue
            # file-register operands must be in the RAM the tests set up
            ok = True
            for m in re.finditer(r"0x([0-9A-Fa-f]+)", text):
                v = int(m.group(1), 16)
                if not (0x1000 <= v < 0x1200):
                    ok = False
            if not ok:
                continue
            cand.append(text)
        # keep only what the assembler takes
        out += assemble_filter(cand)
    return out[:n]

def assemble_filter(cand):
    tmp = tempfile.mkdtemp(prefix="dsdf-")
    keep = list(cand)
    while True:
        src = tmp + "/f.s"
        open(src, "w").write(".text\n" + "".join(" " + t + "\n nop\n" for t in keep))
        r = subprocess.run([a.xc, "-mcpu=33CK64MC105", "-mdfp=" + a.dfp, "-c", src, "-o", tmp + "/f.o"], capture_output=True, text=True)
        errs = set()
        for m in re.finditer(r"f\.s:(\d+): (?:Fatal error|Error)", r.stderr):
            errs.add(int(m.group(1)))
        if not errs:
            break
        # each text is two lines (instruction and nop): the line number says which
        bad = {(e - 2) // 2 for e in errs}
        keep = [t for i, t in enumerate(keep) if i not in bad]
    return keep

def ptr_regs(text):
    regs = set()
    for m in re.finditer(r"\[(?:\+\+|--)?w(\d+)(?:\+w(\d+))?", text):
        regs.add((int(m.group(1)), 'p'))
        if m.group(2):
            regs.add((int(m.group(2)), 'o'))
    return regs

def make_program(texts):
    L = []
    P = L.append
    P(".global __reset\n.global done\n.text\n__reset:")
    P(" mov #0x2900,w15")
    for i, t in enumerate(texts):
        byte = ".b" in t.split()[0]
        t_first = t
        # state: ACC, CORCON, SR first (through w0), then the W registers, last
        P(f".global t{i}_start\n.global t{i}_end")
        # RAM fill
        P(" clr a\n clr b")
        P(" mov #0x1000,w0\n mov #0xACE1,w1")
        P(f" do #255,fl{i}\n mov w1,[w0++]\n mul.uu w1,#31,w2\n add w2,#13,w1\nfl{i}: nop")
        P(f" mov #0x2000,w0\n mov #0x5EED,w1")
        P(f" do #127,fy{i}\n mov w1,[w0++]\n mul.uu w1,#29,w2\n add w2,#11,w1\nfy{i}: nop")
        for n, (reg, _) in enumerate([("ACCAL", 0x22), ("ACCAH", 0x24), ("ACCAU", 0x26), ("ACCBL", 0x28), ("ACCBH", 0x2A), ("ACCBU", 0x2C)]):
            v = rnd.getrandbits(16)
            if reg.endswith("U"):
                v = None  # sign extension of the accumulator's bit 31, so that no overflow is implied
            if v is None:
                P(f" mov {reg[:-1] + 'H'},w0\n asr w0,#15,w0\n mov w0,{reg}")
            else:
                P(f" mov #{v},w0\n mov w0,{reg}")
        corcon = rnd.getrandbits(16) & 0x30F7 | 0x0020 if rnd.random() < 0.7 else 0x0020
        corcon &= ~0x30 if rnd.random() < 0.5 else 0xFFFF
        P(f" mov #{corcon & 0xB0F3},w0\n mov w0,CORCON")
        sr = rnd.getrandbits(16) & 0x010F  # MDB ignores software writes to OA/OB/SA/SB/OAB/SAB
        P(f" mov #{sr},w0\n mov w0,SR")
        ptrs = ptr_regs(t)
        pset = {r for r, k in ptrs if k == 'p'}
        oset = {r for r, k in ptrs if k == 'o'} - pset
        for r in range(16):
            if r in pset:
                macy = re.match(r"(mac|msc|mpy|mpy\.n|ed|edac|movsac|clr)\s", t) and r in (10, 11)
                v = rnd.randrange(0x2010, 0x20E0) if macy else rnd.randrange(0x1010, 0x11E0)
                if not byte:
                    v &= ~1
            elif r in oset:
                v = rnd.choice([0, 2, 4, 6, 8, 0x20, 0xFFFE, 0xFFF0, 0xFFFA, rnd.randrange(0, 64) & ~1])
            elif r == 15:
                v = 0x2900
            elif False:
                pass
            else:
                v = rnd.choice([0, 1, 0xFFFF, 0x8000, 0x7FFF, rnd.getrandbits(16), rnd.getrandbits(16), rnd.getrandbits(8), 0xFF, 0x80])
            if r == 15:
                v &= ~1
            P(f" mov #{v},w{r}")
        P(f"t{i}_start:\n " + t.replace(";", "\n ") + f"\n nop\nt{i}_end:")
        base = 0x2100 + 64 * i
        for r in range(16):
            P(f" mov w{r},0x{base + 2 * r:X}")
        for k, reg in enumerate([0x42, 0x22, 0x24, 0x26, 0x28, 0x2A, 0x2C, 0x44]):
            P(f" mov 0x{reg:X},w0\n mov w0,0x{base + 32 + 2 * k:X}")
        P(f" mov #0x1000,w0\n clr w1\n clr w2\n do #255,ck{i}\n add w1,[w0],w1\n xor w2,[w0++],w2\nck{i}: rlc w2,w2")
        P(f" mov w1,0x{base + 48:X}\n mov w2,0x{base + 50:X}")
        P(f" mov #0x2000,w0\n clr w1\n clr w2\n do #127,cy{i}\n add w1,[w0],w1\n xor w2,[w0++],w2\ncy{i}: rlc w2,w2")
        P(f" mov w1,0x{base + 52:X}\n mov w2,0x{base + 54:X}")
    P("done: bra done")
    return "\n".join(L) + "\n"

def run_batch(texts):
    tmp = tempfile.mkdtemp(prefix="dsdf-")
    src = tmp + "/p.s"
    open(src, "w").write(make_program(texts))
    gld = a.dfp + "/support/dsPIC33C/gld/p33CK64MC105.gld"
    r = subprocess.run([a.xc, "-mcpu=33CK64MC105", "-mdfp=" + a.dfp, "-nostartfiles", "-nodefaultlibs", f"-Wl,-T,{gld},-Map={tmp}/p.map,--no-check-sections", "-o", tmp + "/p.elf", src], capture_output=True, text=True)
    if r.returncode != 0:
        print(r.stderr[:2000]); sys.exit(2)
    b2h = os.path.dirname(a.xc) + "/xc-dsc-bin2hex"
    subprocess.run([b2h, "-a", "-mdfp=" + a.dfp, tmp + "/p.elf"], capture_output=True, check=True)
    mp = open(tmp + "/p.map").read()
    sym = {m.group(2): int(m.group(1), 16) for m in re.finditer(r"^\s+0x([0-9a-f]+)\s+((?:t\d+_(?:start|end))|done)\s*$", mp, re.M)}
    n = len(texts)
    stops = [sym[f"t{i}_{k}"] for i in range(n) for k in ("start", "end")]
    # picsim
    out = subprocess.run([RUN, tmp + "/p.hex", DEV, "--stops", ",".join(f"{s:X}" for s in stops), "--until", f"{sym['done']:X}", "--dump", f"2100:{64 * n}", "--max-cycles", "400000000"], capture_output=True, text=True)
    mine_stops = {}
    seq = [int(m.group(2)) for m in re.finditer(r"^STOP ([0-9A-F]+) (\d+)", out.stdout, re.M)]
    mine_dump = re.search(r"^DUMP 2100 ([0-9a-f ]+)$", out.stdout, re.M)
    # mdb
    lines = ["device dsPIC33CK64MC105", "hwtool sim", f"program {tmp}/p.elf"]
    # MDB's simulator takes few breakpoints at a time: one is set, run to, deleted
    lines += [f"break *0x{sym['t0_start']:X}", "run", "wait 20000"]
    for i in range(n):
        nxt = sym[f"t{i + 1}_start"] if i + 1 < n else sym["done"]
        lines += ["delete", f"break *0x{sym[f't{i}_end']:X}", "run", "wait 20000", "stopwatch",
                  "delete", f"break *0x{nxt:X}", "run", "wait 20000"]
    lines += [f"x /{64 * n}xb 0x2100", "quit"]
    open(tmp + "/m.mdb", "w").write("\n".join(lines) + "\n")
    mo = subprocess.run([a.mdb, tmp + "/m.mdb"], capture_output=True, text=True, stdin=subprocess.DEVNULL, timeout=1800).stdout
    mdb_cyc = [int(x) for x in re.findall(r"cycle count = (\d+)", mo)]
    m = re.search(r"x /%dxb 0x2100\n((?:[0-9a-f]{2}\s+)+)" % (64 * n), mo)
    mdb_bytes = m.group(1).split() if m else []
    if a.keep:
        print('kept', tmp)
        open(tmp + '/mdb.out', 'w').write(mo)
    return tmp, seq, (mine_dump.group(1).split() if mine_dump else []), mdb_cyc, mdb_bytes

NAMES = [f"W{i}" for i in range(16)] + ["SR", "ACCAL", "ACCAH", "ACCAU", "ACCBL", "ACCBH", "ACCBU", "CORCON", "CK1", "CK2", "CK3", "CK4"]
if a.texts:
    texts = [l.strip() for l in open(a.texts) if l.strip() and not l.startswith("#")]
elif a.text:
    texts = [a.text] * a.tests
else:
    texts = gen_texts(a.tests)
bad = 0
done = 0
for b in range(0, len(texts), a.per_run):
    chunk = texts[b:b + a.per_run]
    tmp, seq, mine, mdbc, mdbb = run_batch(chunk)
    # segments: STOP at start (cycles of the preceding setup), then at end (the instruction + nop)
    for i, t in enumerate(chunk):
        errs = []
        if len(mine) < 64 * (i + 1) or len(mdbb) < 64 * (i + 1):
            errs.append(f"missing data (mine {len(mine)}, mdb {len(mdbb)} bytes)")
        else:
            mw = [int(mine[64 * i + 2 * k], 16) | int(mine[64 * i + 2 * k + 1], 16) << 8 for k in range(28)]
            dw = [int(mdbb[64 * i + 2 * k], 16) | int(mdbb[64 * i + 2 * k + 1], 16) << 8 for k in range(28)]
            for k in range(28):
                if mw[k] != dw[k]:
                    errs.append(f"{NAMES[k]}: picsim {mw[k]:04X} mdb {dw[k]:04X}")
        if not a.no_cycles and len(seq) > 2 * i + 1 and len(mdbc) > 2 * i:
            mc, dc = seq[2 * i + 1], mdbc[2 * i]
            if mc != dc:
                errs.append(f"cycles: picsim {mc} mdb {dc}")
        done += 1
        if errs:
            bad += 1
            print(f"DIFF {t}: " + "; ".join(errs[:8]))
print(f"{done} tests, {bad} differ")
sys.exit(1 if bad else 0)
