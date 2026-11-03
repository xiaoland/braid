from pathlib import Path
import xml.etree.ElementTree as E,subprocess,copy,json,hashlib
from PIL import Image,ImageDraw,ImageFont
P=Path(__file__).parent;N='{http://www.w3.org/2000/svg}';E.register_namespace('',N[1:-1]);SIZES=[16,20,24,32,48,64,128,256,512,1024];VARIANTS=[('color','transparent'),('color','white'),('color','black'),('color','badge'),('white','transparent'),('white','black'),('black','transparent'),('black','white')]
def mono(tone,bg,n=None):
 r=E.parse(P/'source/approved-monochrome.svg').getroot();r.set('width','1254');r.set('height','1254');r.find(N+'rect').set('fill','none' if bg=='transparent' else bg);p=r.find(N+'path');p.set('fill',tone);p.set('id','approved-monochrome-silhouette')
 if n:
  stroke=max(0,1.3*1254/n-10)
  for cut in r.find(N+'defs').find(N+'mask').findall(N+'path'):
   cut.set('stroke','black');cut.set('stroke-width',f'{stroke:.3f}');cut.set('stroke-linejoin','round')
  if n<=32:
   r.remove(p);g=E.SubElement(r,N+'g',{'transform':'translate(627 627) scale(1.08) translate(-627 -627)','id':'small-size-optical-scale'});g.append(p)
 title=E.SubElement(r,N+'title');title.text=f'Braid {tone} on {bg} / '+(f'{n}px optical' if n else 'approved standard')
 return E.tostring(r,encoding='unicode')
for tone,bg in VARIANTS:
 name=f'braid-{tone}-on-{bg}'
 if tone!='color':
  f=P/'svg/standard'/(name+'.svg');f.parent.mkdir(parents=True,exist_ok=True);f.write_text(mono(tone,bg))
  for n in SIZES:
   if n<=128:
    f=P/'svg/optical'/str(n)/(name+'.svg');f.parent.mkdir(parents=True,exist_ok=True);f.write_text(mono(tone,bg,n))
 for n in SIZES:
  if tone=='color':src=P/'svg'/('small-optical-color' if n<=32 else 'standard')/(name+'.svg')
  else:src=P/'svg/optical'/str(n)/(name+'.svg') if n<=128 else P/'svg/standard'/(name+'.svg')
  out=P/'png'/name/(name+f'-{n}.png');out.parent.mkdir(parents=True,exist_ok=True)
  subprocess.run(['inkscape',str(src),'--export-filename='+str(out),'--export-width='+str(n),'--export-height='+str(n)],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True)
font=ImageFont.truetype('/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf',17)
s=Image.new('RGB',(1360,1250),'#e7e9ee');d=ImageDraw.Draw(s)
for row,(tone,bg) in enumerate(VARIANTS):
 name=f'braid-{tone}-on-{bg}';y=10+row*154;d.text((12,y+7),f'{tone} / {bg}',font=font,fill='black')
 for col,n in enumerate([16,24,32,64,128,256]):
  x=245+col*180;tile=Image.new('RGBA',(160,120),'#7d8290' if bg=='transparent' else bg if bg!='badge' else '#bec2cb');im=Image.open(P/'png'/name/(name+f'-{n}.png')).convert('RGBA');im=im.resize((112,112),Image.Resampling.NEAREST if n<=32 else Image.Resampling.LANCZOS);tile.alpha_composite(im,(24,4));s.paste(tile.convert('RGB'),(x,y));d.text((x,y+124),f'{n}px'+(' enlarged' if n<=32 else ''),font=font,fill='black')
s.save(P/'preview.png')
s=Image.new('RGB',(900,410),'#e7e9ee');d=ImageDraw.Draw(s)
for row,(tone,bg) in enumerate([('black','white'),('white','black')]):
 d.text((10,10+row*195),f'{tone} / {bg} - actual pixels',font=font,fill='black')
 for i,n in enumerate([16,20,24,32,48,64,128]):
  im=Image.open(P/'png'/f'braid-{tone}-on-{bg}'/f'braid-{tone}-on-{bg}-{n}.png').convert('RGBA');tile=Image.new('RGBA',(115,145),bg);tile.alpha_composite(im,((115-n)//2,(145-n)//2));x=10+i*125;y=40+row*195;s.paste(tile.convert('RGB'),(x,y));d.text((x+10,y+150),str(n),font=font,fill='black')
s.save(P/'actual-size-preview.png')
