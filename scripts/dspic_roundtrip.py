#!/usr/bin/env python3
"""Round-trips random instruction words through picsim's dsPIC disassembler and the MPLAB XC-DSC assembler.

For each random 24-bit word the disassembler (`examples/dspic_dis.rs`) gives assembler text; the text is assembled and the
word it gives must be the word we started from (the encodings of the instructions the decoder accepts are the assembler's).
Branch-like instructions (relative targets, call/goto) are left out: their field is a relocation, not a word the assembler
fixes. Usage: dspic_roundtrip.py [--n N] [--seed S] [--top 0xB8,0xB9] [--show]
"""
import argparse, os, random, re, subprocess, sys, tempfile

ap = argparse.ArgumentParser()
ap.add_argument("--n", type=int, default=20000)
ap.add_argument("--seed", type=int, default=1)
ap.add_argument("--top", default="", help="comma separated top bytes to test (default: all)")
ap.add_argument("--show", type=int, default=25, help="how many mismatches to print")
ap.add_argument("--xc", default="/opt/microchip/xc-dsc/v4.00/bin/xc-dsc-gcc")
ap.add_argument("--dfp", default=os.path.expanduser("~/.mchp_packs/Microchip/dsPIC33CK-MC_DFP/1.11.412/xc16"))
a = ap.parse_args()
root = os.path.dirname(os.path.abspath(__file__)) + "/.."
random.seed(a.seed)
tops = [int(t, 0) for t in a.top.split(",")] if a.top else list(range(256))
SKIP = re.compile(r"^(bra|call|goto|rcall|do|cpb(gt|lt|ne|eq))\b")

words = []
for _ in range(a.n):
    t = random.choice(tops)
    words.append((t << 16 | random.getrandbits(16), random.getrandbits(16)))
subprocess.run(["cargo", "build", "-q", "-p", "picsim", "--example", "dspic_dis"], cwd=root, check=True)
dis = subprocess.run([root + "/target/debug/examples/dspic_dis"], input="\n".join(f"{w:06X} {w2:06X}" for w, w2 in words), capture_output=True, text=True, check=True).stdout.splitlines()
items = []
for (w, w2), line in zip(words, dis):
    n, i, text = line.split("\t")
    if n == "0" or SKIP.match(text):
        continue
    items.append((w, w2, int(n), text))
tmp = tempfile.mkdtemp(prefix="dsrt-")
bad = 0

def assemble(sub):
    """Assembles the texts; returns (stderr, {line: message}, listing path)."""
    src = os.path.join(tmp, "t.s")
    with open(src, "w") as f:
        f.write(".text\n")
        for _, _, _, text in sub:
            f.write(" " + text + "\n")
    lst = os.path.join(tmp, "t.lst")
    if os.path.exists(lst):
        os.remove(lst)
    r = subprocess.run([a.xc, "-mcpu=33CK64MC105", "-mdfp=" + a.dfp, f"-Wa,-ahlns={lst}", "-c", src, "-o", os.path.join(tmp, "t.o")], capture_output=True, text=True)
    errs = {}
    lines = r.stderr.splitlines()
    for i, l in enumerate(lines):
        m = re.search(r"t\.s:(\d+): (?:Fatal error|Error): (.*)", l)
        if m:
            msg = m.group(2)
            if i + 1 < len(lines) and re.search(r"t\.s:\d+:\s+Check", lines[i + 1]):
                msg += " / " + lines[i + 1].split(":", 2)[2].strip()
            errs[int(m.group(1))] = msg
    return r, errs, lst

REJECT = {}
REJ_WHY = {}

def rounds(sub):
    out = []
    while True:
        r, errs, lst = assemble(sub)
        if not errs:
            break
        keep = []
        for i, s in enumerate(sub):
            if (i + 2) in errs:
                out.append((s, None))
                REJECT.setdefault(re.sub(r"'[^']*'", "", errs[i + 2]), []).append(s)
                REJ_WHY[s[3]] = errs[i + 2]
            else:
                keep.append(s)
        if len(keep) == len(sub):
            break
        sub = keep
    if not os.path.exists(lst):
        print('assembler failed:', r.stderr[:2000]); sys.exit(2)
    words_by_line = {}
    cur = None
    for l in open(lst):
        m = re.match(r"^\s+(\d+) [0-9a-f]{6}  ([0-9A-F]{2}) ([0-9A-F]{2}) ([0-9A-F]{2})\s*(.*)", l)
        if m:
            cur = int(m.group(1))
            words_by_line[cur] = [int(m.group(4) + m.group(3) + m.group(2), 16)]
            continue
        m = re.match(r"^\s+(\d+)\s+([0-9A-F]{2}) ([0-9A-F]{2}) ([0-9A-F]{2})\s*$", l)
        if m and cur is not None:
            words_by_line[cur].append(int(m.group(4) + m.group(3) + m.group(2), 16))
    for i, s in enumerate(sub):
        out.append((s, words_by_line.get(i + 2)))
    return out

res = rounds(items)
mism = {}
BENIGN = re.compile(r"must be even|cannot follow a REPEAT|Operand must be between|Register # must be between|Register # must be even|should have even register")
redis_in = []
for (w, w2, n, text), got in res:
    if got is not None:
        redis_in.append(f"{got[0]:06X} {got[1] if len(got) > 1 else 0:06X}")
redis = subprocess.run([root + "/target/debug/examples/dspic_dis"], input="\n".join(redis_in), capture_output=True, text=True, check=True).stdout.splitlines()
ri = iter(redis)
for (w, w2, n, text), got in res:
    if got is None:
        key = "assembler rejects"
        why = REJ_WHY.get(text, "")
        if BENIGN.search(why):
            continue
    else:
        back = next(ri).split("\t")[2]
        if back == text:
            continue
        key = "different meaning"
    mism.setdefault(key, []).append((w, w2, n, text, got))
total = len(items)
if a.show:
    for k, v in sorted(REJECT.items(), key=lambda kv: -len(kv[1])):
        print(f"-- assembler: {k.strip()} ({len(v)}), e.g. {v[0][3]}")
print(f"{total} decoded words tested")
for k, v in mism.items():
    print(f"== {k}: {len(v)}")
    seen = set()
    shown = 0
    for w, w2, n, text, got in sorted(v):
        fam = (w >> 16, text.split()[0])
        if fam in seen:
            continue
        seen.add(fam)
        if shown >= a.show:
            break
        shown += 1
        g = " ".join(f"{x:06X}" for x in got) if got else REJ_WHY.get(text, "-")[:90]
        print(f"  {w:06X}{(' ' + format(w2, '06X')) if n == 2 else ''}  {text:<40} assembler: {g}")
sys.exit(1 if mism else 0)
