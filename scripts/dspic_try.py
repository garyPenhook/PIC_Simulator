#!/usr/bin/env python3
"""Runs an assembly snippet in MDB and in picsim and prints W0..W15, SR, accumulators and the cycle count of both.

Usage: dspic_try.py 'mov #0xF0,w0; mov #0x20,w1; add w0,w1,w2'   (statements separated by ';'; labels as `lab:`)
The snippet runs between the labels `start` and `end` (cycles are the stopwatch between them); W15 starts at 0x2900.
"""
import os, re, subprocess, sys, tempfile
xc = "/opt/microchip/xc-dsc/v4.00/bin/xc-dsc-gcc"
dfp = os.path.expanduser("~/.mchp_packs/Microchip/dsPIC33CK-MC_DFP/1.11.412/xc16")
mdb = "/opt/microchip/mplabx/v6.35/mplab_platform/bin/mdb.sh"
root = os.path.dirname(os.path.abspath(__file__)) + "/.."
body = "\n".join(" " + s.strip() if not s.strip().endswith(":") else s.strip() for s in " ".join(sys.argv[1:]).split(";") if s.strip())
tmp = tempfile.mkdtemp(prefix="dstry-")
src = f".global __reset\n.global start\n.global end\n.global done\n.text\n__reset:\n mov #0x2900,w15\nstart:\n{body}\nend:\n nop\ndone: bra done\n"
open(tmp + "/p.s", "w").write(src)
gld = dfp + "/support/dsPIC33C/gld/p33CK64MC105.gld"
r = subprocess.run([xc, "-mcpu=33CK64MC105", "-mdfp=" + dfp, "-nostartfiles", "-nodefaultlibs", f"-Wl,-T,{gld},-Map={tmp}/p.map,--no-check-sections", "-o", tmp + "/p.elf", tmp + "/p.s"], capture_output=True, text=True)
if r.returncode:
    print(r.stderr); sys.exit(1)
subprocess.run([os.path.dirname(xc) + "/xc-dsc-bin2hex", "-a", "-mdfp=" + dfp, tmp + "/p.elf"], capture_output=True, check=True)
mp = open(tmp + "/p.map").read()
sym = {m.group(2): int(m.group(1), 16) for m in re.finditer(r"^\s+0x([0-9a-f]+)\s+(start|end|done)\s*$", mp, re.M)}
subprocess.run(["cargo", "build", "-q", "-p", "picsim", "--example", "dspic_run"], cwd=root, capture_output=True)
mine = subprocess.run([root + "/target/debug/examples/dspic_run", tmp + "/p.hex", root + "/devices/DSPIC33CK64MC105.toml", "--stops", f"{sym['start']:X},{sym['end']:X}", "--until", f"{sym['done']:X}", "--dump", "0:96"], capture_output=True, text=True).stdout
open(tmp + "/m.mdb", "w").write(f"device dsPIC33CK64MC105\nhwtool sim\nprogram {tmp}/p.elf\nbreak *0x{sym['start']:X}\nrun\nwait 20000\ndelete\nbreak *0x{sym['end']:X}\nrun\nwait 20000\nstopwatch\ndelete\nbreak *0x{sym['done']:X}\nrun\nwait 20000\nx /96xb 0x0\nquit\n")
mo = subprocess.run([mdb, tmp + "/m.mdb"], capture_output=True, text=True, stdin=subprocess.DEVNULL).stdout
cyc = re.findall(r"cycle count = (\d+)", mo)
m = re.search(r"x /96xb 0x0\n((?:[0-9a-f]{2}\s+)+)", mo)
mdbb = m.group(1).split() if m else []
mb = re.search(r"^DUMP 0 ([0-9a-f ]+)$", mine, re.M).group(1).split()
mcyc = [int(x) for x in re.findall(r"^STOP \S+ (\d+)", mine, re.M)]
names = [f"W{i}" for i in range(16)] + ["SPLIM", "ACCAL", "ACCAH", "ACCAU", "ACCBL", "ACCBH", "ACCBU", "PCL", "PCH", "DSRPAG", "DSWPAG", "RCOUNT", "DCOUNT", "DOSTARTL", "DOSTARTH", "DOENDL", "DOENDH", "SR", "CORCON"]
def w(b, i): return int(b[2 * i], 16) | int(b[2 * i + 1], 16) << 8
print(f"{'':10}{'picsim':>8}{'mdb':>8}")
for i, n in enumerate(names):
    a_, b_ = w(mb, i), (w(mdbb, i) if mdbb else -1)
    flag = "" if a_ == b_ else "   <<<"
    if i < 16 and a_ == 0 and b_ == 0 and i not in (0, 1, 2, 3): continue
    print(f"{n:10}{a_:8X}{b_ & 0xFFFFF:8X}{flag}")
print(f"cycles    {mcyc[1] if len(mcyc) > 1 else '?':>8}{cyc[0] if cyc else '?':>8}  (start..end, including the nop)")
