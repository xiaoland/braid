from pathlib import Path
import json,hashlib,xml.etree.ElementTree as E
from PIL import Image
import numpy as np
from scipy.ndimage import label
P=Path(__file__).parent;S=[16,20,24,32,48,64,128,256,512,1024];results=[]
def img(t,b,n):return np.array(Image.open(P/'png'/f'braid-{t}-on-{b}'/f'braid-{t}-on-{b}-{n}.png').convert('RGBA'))
for n in S:
 bt=img('black','transparent',n);wt=img('white','transparent',n);bw=img('black','white',n);wb=img('white','black',n)
 negative=bt[:,:,3]<128;l,c=label(negative);sizes=np.bincount(l.ravel())[1:].tolist()
 results.append({'size':n,'alpha_difference_max':int(np.abs(bt[:,:,3].astype(int)-wt[:,:,3].astype(int)).max()),'solid_complement_error_max':int(np.abs(bw[:,:,:3].astype(int)+wb[:,:,:3].astype(int)-255).max()),'alpha_vs_solid_error_max':int(np.abs(bt[:,:,3].astype(int)-wb[:,:,0].astype(int)).max()),'negative_regions_4_connected':c,'negative_region_areas':sizes})
imgs=[]
for p in (P/'png').rglob('*.png'):
 im=Image.open(p);n=int(p.stem.rsplit('-',1)[1]);assert im.size==(n,n);imgs.append(str(p.relative_to(P)))
svg=[]
for p in (P/'svg').rglob('*.svg'):
 root=E.parse(p).getroot();bad=[]
 for e in root.iter():
  if e.tag.split('}')[-1] in ['image','text']:bad.append(e.tag)
  for k,v in e.attrib.items():
   if k.endswith('href') and not v.startswith('#'):bad.append(v)
 assert not bad,(p,bad);svg.append(str(p.relative_to(P)))
assert len(imgs)==80 and len(svg)==40
assert all(q['alpha_difference_max']==0 and q['solid_complement_error_max']==0 and q['alpha_vs_solid_error_max']==0 and q['negative_regions_4_connected']==3 for q in results)
baseline=json.loads((P/'source/color-v2-hashes.json').read_text());unchanged=all(hashlib.sha256((P/k).read_bytes()).hexdigest()==v for k,v in baseline.items());assert unchanged
report={'color_v2_byte_identical_files':len(baseline),'color_v2_unchanged':unchanged,'png_count':len(imgs),'svg_count':len(svg),'monochrome_checks':results,'svg_no_images_text_external_references':True,'note':'Negative-region check uses threshold alpha 128 and 4-neighbour connectivity; standard has two enclosed counters plus exterior-connected lower openings.'}
(P/'QA.json').write_text(json.dumps(report,indent=2))
manifest=[]
for p in sorted(P.rglob('*')):
 if p.is_file() and p.name!='manifest.json':manifest.append({'path':str(p.relative_to(P)),'bytes':p.stat().st_size,'sha256':hashlib.sha256(p.read_bytes()).hexdigest()})
(P/'manifest.json').write_text(json.dumps(manifest,indent=2))
print(json.dumps(report,indent=2))
