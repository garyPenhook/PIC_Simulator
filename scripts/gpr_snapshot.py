#!/usr/bin/env python3
"""Full GPR-snapshot differential gate: basil-sim against MPLAB X MDB.

For every selected compiler test program, build it, run it to `__halt` in both
engines and compare every general-purpose RAM byte listed in the device file, not
only the declared result variables. A mismatch is reported with the address, the
program variable that owns it (from the compiler's .S listing) and both values,
and the cycle counts of both engines are shown beside it.

Usage: gpr_snapshot.py --device 18F27Q10 [--flags=-O0] [--limit N] [prog.bas ...]
Exit status is 1 when any program differs (it does not hide differences).
Requires the sibling Basic_Compiler checkout, release binaries and MPLAB X.
"""
import argparse, glob, json, os, re, shutil, subprocess, sys, tempfile, tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SUITE = {"18F27Q10": "exec", "18F16Q41": "exec_q41", "18F25K22": "exec_k22", "18F45K22": "exec_k22",
         "16F17146": "exec_p16", "16F13145": "exec_p13", "16F13245": "exec_p132", "16F15244": "exec_p152",
         "16F18056": "exec_p180", "16F18346": "exec_p183"}


def ranges(device):
    d = tomllib.load(open(ROOT / "devices" / f"{device}.toml", "rb"))
    spans = sorted((g["start"], g["end"]) for g in d["memory"]["data"]["gpr"])
    out = []
    for s, e in spans:
        if out and s <= out[-1][1]:
            out[-1][1] = max(out[-1][1], e)
        else:
            out.append([s, e])
    return out


def retarget(text, device):
    """Same header rewrite as the compiler's exec-test.py, for a part other than 18F27Q10."""
    if device == "18F27Q10":
        return text
    text = re.sub(r"^(\s*Device\s*=\s*)\S+", lambda m: m.group(1) + device, text, count=1, flags=re.M | re.I)
    if device.startswith(("16F", "16LF")):
        rst = "HFINT32" if device.startswith(("16F183", "16LF183", "16F153", "16LF153")) else "HFINTOSC_32MHz"
        text = re.sub(r"^[ \t]*Config[ \t].*$", f"Config FEXTOSC = OFF, RSTOSC = {rst}, WDTE = OFF, LVP = ON", text, flags=re.M | re.I)
        text = re.sub(r"^(\s*Clock\s*=\s*)64\b", lambda m: m.group(1) + "32", text, flags=re.M | re.I)
    elif "K22" in device:
        text = re.sub(r"^[ \t]*Config[ \t].*(RSTOSC|FEXTOSC).*$", "Config FOSC = ECHPIO6, WDTEN = OFF, LVP = ON", text, flags=re.M | re.I)
    return text


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--compiler-root", type=Path, default=ROOT.parent / "Basic_Compiler")
    ap.add_argument("--device", default="18F27Q10", choices=SUITE)
    ap.add_argument("--flags", default="")
    ap.add_argument("--limit", type=int)
    ap.add_argument("--json", type=Path)
    ap.add_argument("--keep", action="store_true")
    ap.add_argument("files", nargs="*", type=Path)
    a = ap.parse_args()
    comp = a.compiler_root.resolve()
    basil, sim = comp / "target/release/basil", ROOT / "target/release/basil-sim"
    mdb = sorted(glob.glob("/opt/microchip/mplabx/v*/mplab_platform/bin/mdb.sh"))[-1]
    work = Path(tempfile.mkdtemp(prefix="gpr-snap-"))
    if not a.keep:
        import atexit
        atexit.register(shutil.rmtree, work, ignore_errors=True)
    rng = ranges(a.device)
    files = a.files or sorted((comp / "tests" / SUITE[a.device]).glob("*.bas"))
    jobs, skipped = [], []
    for src in files:
        text = src.read_text()
        name = src.stem
        only = re.search(r"^'\s*devices:(.*)$", text, re.M)
        if only and a.device not in re.split(r"[,\s]+", only.group(1).strip()):
            skipped.append((name, "device list"))
        elif re.search(r"^'\s*(engine:|stim:|dma:)", text, re.M):
            skipped.append((name, "not a two-engine program"))
        else:
            jobs.append((src, name, text))
    if a.limit:
        jobs = jobs[: a.limit]
    built = []
    for src, name, text in jobs:
        for cf in re.findall(r'^\s*CFile\s+"([^"]+)"', text, re.M | re.I):
            shutil.copy(src.parent / cf, work)
        for inc in re.findall(r'^\s*Include\s+"([^"]+)"', text, re.M | re.I):
            if (src.parent / (inc + ".bas")).exists():
                shutil.copy(src.parent / (inc + ".bas"), work)
        path = work / f"{name}.bas"
        path.write_text(retarget(text, a.device))
        r = subprocess.run([basil, "--keep-vars", *a.flags.split(), str(path), "-o", str(work / f"{name}.hex")], capture_output=True, text=True)
        asm = work / f"{name}.S"
        if r.returncode or not asm.exists():
            skipped.append((name, "compile failed" if r.returncode else "no .S (C program)"))
            continue
        asm_text = asm.read_text()
        halt = re.search(r"^;SYM __halt 0x([0-9A-F]+)", asm_text, re.M)
        owners = sorted((int(m.group(2), 16), int(m.group(3) or 1), m.group(1))
                        for m in re.finditer(r"^(\w+) equ 0x([0-9A-F]+)(?:  ; .*\((\d+) bytes?\))?", asm_text, re.M))
        if halt:
            built.append((name, int(halt.group(1), 16), owners))
        else:
            skipped.append((name, "no __halt"))
    # MDB: one session for all programs.
    script = [f"device PIC{a.device}", "hwtool SIM"]
    for name, halt, _ in built:
        script += [f"program {work / (name + '.hex')}", f"break *0x{halt:X}", "run", "wait 300000", "stopwatch"]
        script += [f"x /{e - s}xb 0x{s:X}" for s, e in rng]
        script += [f"echo END_{name}", "delete"]
    script.append("quit")
    (work / "run.mdb").write_text("\n".join(script) + "\n")
    out = subprocess.run([mdb, str(work / "run.mdb")], capture_output=True, text=True, timeout=7200).stdout
    chunks = re.split(r"^/\*END_(\S+?)\*/\s*$", out, flags=re.M)
    mdb_res = {}
    for i in range(1, len(chunks) - 1, 2):
        body = chunks[i - 1]
        cy = re.findall(r"Stopwatch cycle count = (\d+)", body)
        data = bytearray()
        for line in body.splitlines():
            if re.fullmatch(r"(?:[0-9a-f]{2}\s+)+", line + " ") and not line.lstrip().startswith(("Stop", "Single")):
                data += bytes(int(x, 16) for x in line.split())
        mdb_res[chunks[i]] = (int(cy[-1]) if cy else None, bytes(data))
    want = sum(e - s for s, e in rng)
    results, bad = [], 0
    for name, halt, owners in built:
        cyc_m, data_m = mdb_res.get(name, (None, b""))
        args = [sim, work / f"{name}.hex", "--device", a.device, "--asm", work / f"{name}.S",
                "--dump", ",".join(f"0x{s:X}:{e - s}" for s, e in rng)]
        r = subprocess.run(list(map(str, args)), capture_output=True, text=True, timeout=3600)
        m = re.search(r"stop: (\w+).*cycles: (\d+)", r.stdout)
        data_s = bytearray()
        for line in r.stdout.splitlines():
            if " = " in line and "(" in line:
                data_s += bytes(int(x, 16) for x in line.split("(")[1].rstrip(")").split())
        row = {"program": name, "sim_cycles": int(m.group(2)) if m else None, "mdb_cycles": cyc_m}
        if len(data_m) != want or len(data_s) != want or not m or m.group(1) != "Halt":
            row["status"] = "error"
            row["detail"] = f"sim {len(data_s)}B stop={m.group(1) if m else None}, mdb {len(data_m)}B, want {want}B"
        else:
            diffs, pos = [], 0
            for s, e in rng:
                for k in range(e - s):
                    if data_s[pos + k] != data_m[pos + k]:
                        addr = s + k
                        own = next((n for st, sz, n in owners if st <= addr < st + sz), "?")
                        diffs.append({"addr": f"0x{addr:X}", "var": own, "sim": data_s[pos + k], "mdb": data_m[pos + k]})
                pos += e - s
            row["status"] = "same" if not diffs else "differs"
            row["diffs"] = diffs
        bad += row["status"] != "same"
        results.append(row)
        extra = "" if row["status"] == "same" else f"  {len(row.get('diffs', []))} bytes" if row["status"] == "differs" else f"  {row['detail']}"
        print(f"{row['status']:7} {name}  cycles sim={row['sim_cycles']} mdb={cyc_m}{extra}", flush=True)
        for d in row.get("diffs", [])[:8]:
            print(f"          {d['addr']} {d['var']}: sim={d['sim']:02X} mdb={d['mdb']:02X}")
    print(f"{len(results) - bad} of {len(results)} programs have identical GPR snapshots; {len(skipped)} skipped")
    if a.json:
        a.json.write_text(json.dumps({"device": a.device, "flags": a.flags, "results": results, "skipped": skipped}, indent=1) + "\n")
    return int(bad > 0)


if __name__ == "__main__":
    sys.exit(main())
