import json, struct, sys
# usage: geo.py in.json out.bin
# out: sequence of polylines: u32 id (state or state*1000+county FIPS), u16 npts, i32 lon0, i32 lat0 (1e-4 deg), then (npts-1) * (i16 dlon, i16 dlat)
src, dst = sys.argv[1], sys.argv[2]
d = json.load(open(src, encoding='latin-1'))
out = bytearray(); nl = 0
def emit(pts, fid):
    global nl, out
    q = [(round(x*1e4), round(y*1e4)) for x, y in pts]
    # dedupe consecutive
    r = [q[0]]
    for p in q[1:]:
        if p != r[-1]: r.append(p)
    i = 0
    while i < len(r) - 1:
        seg = [r[i]]
        j = i + 1
        while j < len(r) and len(seg) < 65000:
            dx, dy = r[j][0]-seg[-1][0], r[j][1]-seg[-1][1]
            if abs(dx) > 32767 or abs(dy) > 32767: break
            seg.append(r[j]); j += 1
        if len(seg) >= 2:
            out += struct.pack('<IHii', fid, len(seg), *seg[0])
            for a, b in zip(seg, seg[1:]): out += struct.pack('<hh', b[0]-a[0], b[1]-a[1])
            nl += 1
        i = j - 1 if j - 1 > i else i + 1
for f in d['features']:
    pr = f['properties']
    fid = int(pr['STATE']) * 1000 + int(pr['COUNTY']) if 'COUNTY' in pr else int(pr['STATE'])
    g = f['geometry']
    polys = [g['coordinates']] if g['type'] == 'Polygon' else g['coordinates']
    for poly in polys:
        for ring in poly: emit(ring, fid)
open(dst, 'wb').write(out); print(dst, nl, len(out))
