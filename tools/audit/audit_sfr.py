#!/usr/bin/env python3
"""Compare the 8-bit SFR tables of a data sheet (pdftotext -layout output) with a device description: implemented mask (the
bits that have an Access entry) and reset values, matched by address. Writes OUT and OUT.diff (JSON).
Usage: audit_sfr.py DATASHEET.txt devices/PART.toml OUT.json
Caveats: a generic register page (xCON, PWMx...) describes every instance with one table; a data sheet that covers a larger part
(Q10: 18F27 and 18F47) lists bits the smaller part lacks. Treat each difference as a candidate and read the page before changing a device file."""
import re,sys,tomllib,json
txt,tomlf,out=sys.argv[1:4]
lines=open(txt,errors='replace').read().split('\n')
d=tomllib.load(open(tomlf,'rb'))
byaddr={}
for s in d['sfr']:
    if s.get('width')==8: byaddr.setdefault(s['addr'],[]).append(s)
def toks(l):
    return [(m.start()+len(m.group())/2,m.group()) for m in re.finditer(r'\S+',l)]
res=[]
i=0
while i<len(lines):
    m=re.match(r'\s*(?:Name|Names?):\s+(\S+)',lines[i])
    if not m: i+=1; continue
    name=m.group(1); addr=None; j=i+1
    for k in range(i+1,min(i+4,len(lines))):
        a=re.match(r'\s*(?:Address|Offset):\s+(.*)',lines[k])
        if a: addr=[int(x,16) for x in re.findall(r'0[xX]([0-9A-Fa-f]+)',a.group(1))]; 
        if a: j=k+1; break
    if not addr: i+=1; continue
    # bit row
    b=None
    for k in range(j,min(j+12,len(lines))):
        if re.match(r'\s*Bit\s+(\d+\s+){7}\d+\s*$',lines[k]) or re.match(r'\s*Bit\s+7\s+6\s+5\s+4\s+3\s+2\s+1\s+0',lines[k]): b=k;break
    if b is None: i+=1; continue
    cols=[(p,int(t)) for p,t in toks(lines[b]) if t.isdigit()]
    if [c[1] for c in cols]!=[7,6,5,4,3,2,1,0]: i+=1; continue
    acc=rst=None
    for k in range(b+1,min(b+8,len(lines))):
        if re.match(r'\s*Access\b',lines[k]) and acc is None: acc=k
        if re.match(r'\s*Reset\b',lines[k]) and rst is None: rst=k
    if acc is None or rst is None: i+=1; continue
    def assign(line,skip):
        t=toks(line)[1:]
        o={}
        for p,x in t:
            bit=min(cols,key=lambda c:abs(c[0]-p))[1]
            o[bit]=x
        return o
    A=assign(lines[acc],1); R=assign(lines[rst],1)
    mask=sum(1<<b for b in A)
    por=''.join(R.get(7-n,'-') for n in range(8))
    res.append(dict(name=name,addr=addr,mask=mask,por=por,access={b:A[b] for b in A}))
    i=rst
json.dump(res,open(out,'w'))
print(len(res),'tables')
bad_mask=[];bad_por=[];unmatched=[]
for r in res:
    for a in r['addr']:
        for s in byaddr.get(a,[]):
            if s.get('implemented') is not None and s['implemented']!=r['mask']:
                bad_mask.append((s['name'],r['name'],hex(a),s['implemented'],r['mask']))
            sp=s.get('por')
            if sp:
                # compare reset on implemented bits; x/u/q unknown ok
                dp=r['por']
                for n in range(8):
                    if r['mask']>>(7-n)&1==0: continue
                    c1,c2=sp[n],dp[n]
                    if c1!=c2 and not({c1,c2}&{'x','u','q'}):
                        bad_por.append((s['name'],r['name'],hex(a),sp,dp)); break
json.dump(dict(mask=bad_mask,por=bad_por),open(out+'.diff','w'),indent=0)
print('mask diffs',len(bad_mask),'por diffs',len(bad_por))
