#!/usr/bin/env python3
"""Compare the register file summary table of an older PIC18 data sheet (pdftotext -layout output of its pages; one row per
register: `F80h  PORTA  bit names ...  xxxx xxxx`, the last field being the value on POR/BOR) with a device description: the
implemented mask (the bits not shown as '-' in that value) and the reset value, matched by address. For data sheets such as
DS40001412H (PIC18(L)F2X/4XK22, Table 5-2 p.79-83), which have no per-register `Name: / Address:` pages for audit_sfr.py.
Usage: audit_summary.py TABLE.txt devices/PART.toml OUT.json   (writes OUT and OUT.diff, the same format as audit_sfr.py)
Caveats: the table covers the whole family, so it lists registers and bits the smaller parts lack (a footnote marks them; the
script does not read footnotes), and a value with 'q' depends on a condition. Treat each difference as a candidate and read the
page before changing a device file."""
import re, sys, tomllib, json

txt, tomlf, out = sys.argv[1:4]
d = tomllib.load(open(tomlf, 'rb'))
byaddr = {}
for s in d['sfr']:
    if s.get('width') == 8:
        byaddr.setdefault(s['addr'], []).append(s)

row = re.compile(r'^\s*([0-9A-F]{3})h\s+(\S+?)(?:\(\d+(?:,\s*\d+)*\))?\s.*?\s([-01xuq]{4})\s([-01xuq]{4})\s*$')
res = []
for line in open(txt, errors='replace'):
    m = row.match(line)
    if not m:
        continue
    addr, name, por = int(m.group(1), 16), m.group(2), m.group(3) + m.group(4)
    mask = sum(1 << (7 - i) for i, c in enumerate(por) if c != '-')
    res.append(dict(name=name, addr=[addr], mask=mask, por=por))
json.dump(res, open(out, 'w'), indent=1)

diff = {"mask": [], "por": []}
for r in res:
    a = r['addr'][0]
    for s in byaddr.get(a, []):
        imp = s.get('implemented', 255)
        if imp != r['mask']:
            diff["mask"].append(dict(name=s['name'], addr=a, sheet=r['mask'], desc=imp))
        dp = s.get('por')
        if dp is not None and dp.replace('q', 'x') != r['por'].replace('q', 'x'):
            diff["por"].append(dict(name=s['name'], addr=a, sheet=r['por'], desc=dp))
json.dump(diff, open(out + '.diff', 'w'), indent=1, sort_keys=True)
print(f"{len(res)} rows read, {len(diff['mask'])} mask and {len(diff['por'])} reset differences")
