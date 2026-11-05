#!/bin/sh
# Compare devices/*.toml with their data sheets and fail on any difference that is not in the committed baseline.
# Usage: tools/audit/run.sh [--update]      (--update rewrites the baselines after you have read the new differences)
# Needs pdftotext and the data sheet PDFs; PDFDIR (default ~/Documents/Microchip/PIC, searched recursively) holds them.
# Baselines (baseline/<part>.json) list the differences that are known and either not yet fixed or not errors (see docs/REVIEW-FINDINGS.md).
set -e
cd "$(dirname "$0")/../.."
PDFDIR=${PDFDIR:-$HOME/Documents/Microchip/PIC}
w=$(mktemp -d); trap 'rm -rf "$w"' EXIT
rc=0
# part:data-sheet file pattern
for pair in "18F27Q10:*DS40002043.pdf" "18F16Q41:*DS40002214.pdf"; do
    part=${pair%%:*}; pat=${pair#*:}
    pdf=$(find "$PDFDIR" -name "$pat" 2>/dev/null | head -1)
    [ -n "$pdf" ] || { echo "skip $part: no data sheet matching $pat in $PDFDIR"; continue; }
    pdftotext -layout "$pdf" "$w/$part.txt"
    python3 tools/audit/audit_sfr.py "$w/$part.txt" "devices/$part.toml" "$w/$part.json" >/dev/null
    python3 - "$w/$part.json.diff" "tools/audit/baseline/$part.json" "${1:-}" <<'PY' || rc=1
import json,sys
new=json.load(open(sys.argv[1])); path=sys.argv[2]
if sys.argv[3]=="--update":
    import os; os.makedirs(os.path.dirname(path),exist_ok=True); json.dump(new,open(path,'w'),indent=1,sort_keys=True); print("updated",path); sys.exit(0)
try: old=json.load(open(path))
except FileNotFoundError: old={"mask":[],"por":[]}
bad=0
for k in ("mask","por"):
    have={json.dumps(x,sort_keys=True) for x in old.get(k,[])}
    for x in new.get(k,[]):
        if json.dumps(x,sort_keys=True) not in have: print("NEW",k,"difference:",x); bad+=1
print(path,"new differences:",bad)
sys.exit(1 if bad else 0)
PY
done
exit $rc
