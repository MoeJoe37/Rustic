#!/usr/bin/env python3
"""Rebuild original CC0 CFF and renamed OFL Noto shaping fixtures.

Requires FontTools 4.61.1 and Brotli 1.1.0. Source digests are verified on
subsequent runs; this tool never downloads fonts.
"""
from pathlib import Path
import hashlib
import json
import argparse
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.t2CharStringPen import T2CharStringPen
from fontTools.feaLib.builder import addOpenTypeFeaturesFromString
from fontTools.ttLib import TTFont
from fontTools import subset
from fontTools.cffLib.CFFToCFF2 import convertCFFToCFF2
ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / 'crates/rustic-text/tests/fonts/shaping'
def sha(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()
def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--noto-root',type=Path,default=Path('/usr/share/fonts/truetype/noto'))
    args=parser.parse_args()
    OUT.mkdir(parents=True,exist_ok=True)
    old=json.loads((OUT/'manifest.json').read_text()) if (OUT/'manifest.json').exists() else None
    order=['.notdef','space','x','A','V','f','i','fi','acutecomb','zero','hyphen']
    widths=[500,250,500,600,600,300,200,450,0,550,300]
    fb=FontBuilder(1000,isTTF=False)
    fb.setupGlyphOrder(order)
    fb.setupCharacterMap({32:'space',120:'x',65:'A',86:'V',102:'f',105:'i',0x301:'acutecomb',48:'zero',45:'hyphen',0x2010:'hyphen'})
    fb.setupHorizontalMetrics({n:(w,0) for n,w in zip(order,widths)})
    fb.setupHorizontalHeader(ascent=800,descent=-200)
    fb.setupNameTable({'familyName':'Rustic CFF Test','styleName':'Regular','psName':'RusticCFFTest-Regular','fullName':'Rustic CFF Test Regular','version':'Version 1.000'})
    fb.setupOS2(sTypoAscender=800,sTypoDescender=-200,usWinAscent=800,usWinDescent=200,sxHeight=500,sCapHeight=700)
    fb.setupPost()
    chars={}
    for n,w in zip(order,widths):
        pen=T2CharStringPen(w,None)
        if n!='space':
            if n=='acutecomb':
                pen.moveTo((0,0));pen.lineTo((100,150));pen.lineTo((170,150));pen.lineTo((70,0));pen.closePath()
            else:
                pen.moveTo((0,0));pen.lineTo((max(w-100,100),0));pen.curveTo((w,180),(w,500),(max(w-100,100),700));pen.lineTo((0,700));pen.closePath()
        chars[n]=pen.getCharString()
    fb.setupCFF('RusticCFFTest-Regular',{'FullName':'Rustic CFF Test Regular','FamilyName':'Rustic CFF Test','Weight':'Regular'},chars,{})
    addOpenTypeFeaturesFromString(fb.font,'''
        languagesystem DFLT dflt;
        feature liga { sub f i by fi; } liga;
        feature kern { pos A V -80; } kern;
        markClass acutecomb <anchor 0 0> @TOP;
        feature mark { pos base x <anchor 250 600> mark @TOP; } mark;
    ''')
    fb.font['head'].created=fb.font['head'].modified=2082844800
    otf=OUT/'RusticCFFTest.otf';fb.save(otf)
    for flavor in ('woff','woff2'):
        font=TTFont(otf,recalcTimestamp=False);font.flavor=flavor;font.save(OUT/('RusticCFFTest.'+flavor))
    font=TTFont(otf,recalcTimestamp=False);convertCFFToCFF2(font);font.save(OUT/'RusticCFF2Test.otf')
    sources=[]
    for file,label,ranges in [('NotoSansArabic-Regular.ttf','Arabic',[(0x20,0x7e),(0x600,0x6ff),(0x200c,0x200d)]),('NotoSansDevanagari-Regular.ttf','Devanagari',[(0x20,0x7e),(0x900,0x97f),(0x200c,0x200d)])]:
        path=args.noto_root/file;digest=sha(path)
        if old:
            known=next(s for s in old['sources'] if s['path']==file)
            if known['sha256']!=digest:raise ValueError('source digest changed: '+file)
        font=TTFont(path,recalcTimestamp=False)
        version=font['name'].getDebugName(5)
        opts=subset.Options();opts.layout_features=['*'];opts.recalc_timestamp=False
        sub=subset.Subsetter(options=opts);sub.populate(unicodes=[n for a,b in ranges for n in range(a,b+1)]);sub.subset(font)
        family='Rustic '+label+' Fixture'
        for r in font['name'].names:
            value={1:family,2:'Regular',3:family+' 1.0',4:family+' Regular',6:family.replace(' ','')+'-Regular',16:family,17:'Regular'}.get(r.nameID)
            if value:r.string=value.encode(r.getEncoding())
        font['head'].created=font['head'].modified=2082844800
        font.save(OUT/(label+'.ttf'))
        sources.append({'path':file,'sha256':digest,'version':version,'unicode_ranges':ranges,'license':'OFL-1.1','upstream':'https://github.com/notofonts/noto-fonts'})
    outputs={p.name:sha(p) for p in sorted(OUT.iterdir()) if p.suffix in ('.ttf','.otf','.woff','.woff2')}
    (OUT/'manifest.json').write_text(json.dumps({'generator':'tools/generate_shaping_fonts.py','fonttools':'4.61.1','brotli':'1.1.0','original_license':'CC0-1.0','sources':sources,'outputs':outputs,'notices':{n:sha(OUT/n) for n in ['OFL-LICENSE.txt','README.md']}},indent=2)+'\n')
if __name__=='__main__':main()
