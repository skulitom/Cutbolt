"""Original abstract outlines and authored layout tables; generated fonts stay external."""
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.feaLib.builder import addOpenTypeFeaturesFromString

CHARS='fi eABX()0123\u00a0\u0301\u0431\u05d0\u05d1\u05d2\u05d3\u0628\u062a\u064e\u0915\u093f\u094d\u0e01\u0e48\u4e2d\U00010400'
EXTRAS=['fi','beh_initial','beh_medial','beh_final','teh_initial','teh_medial','teh_final','ka_half','cyr_local']
ORDER=['.notdef']+[f'u{ord(c):04x}' for c in CHARS]+EXTRAS
FEATURES='''languagesystem DFLT dflt;
languagesystem latn dflt;
languagesystem hebr dflt;
languagesystem arab dflt;
languagesystem deva dflt;
languagesystem dev2 dflt;
languagesystem thai dflt;
languagesystem cyrl dflt;
languagesystem cyrl SRB;
feature liga {sub u0066 u0069 by fi;} liga;
feature kern {pos u0041 u0042 -100;} kern;
feature init {sub u0628 by beh_initial; sub u062a by teh_initial;} init;
feature medi {sub u0628 by beh_medial; sub u062a by teh_medial;} medi;
feature fina {sub u0628 by beh_final; sub u062a by teh_final;} fina;
feature half {sub u0915 u094d by ka_half;} half;
feature locl { script cyrl; language SRB; sub u0431 by cyr_local; } locl;
markClass u0301 <anchor 50 0> @ABOVE;
markClass u064e <anchor 50 0> @ABOVE;
markClass u0e48 <anchor 50 0> @ABOVE;
feature mark {
pos base u0065 <anchor 300 700> mark @ABOVE;
pos base u0628 <anchor 300 700> mark @ABOVE;
pos base beh_initial <anchor 300 700> mark @ABOVE;
pos base beh_medial <anchor 300 700> mark @ABOVE;
pos base beh_final <anchor 300 700> mark @ABOVE;
pos base u0e01 <anchor 300 700> mark @ABOVE;
} mark;
'''

def make_font(path,variant=0,missing=(),overrides=None,extra_features=''):
    builder=FontBuilder(1000,isTTF=True);builder.setupGlyphOrder(ORDER)
    builder.setupCharacterMap({ord(c):f'u{ord(c):04x}' for c in CHARS if ord(c) not in missing})
    glyphs={};metrics={};geometry={}
    for i,name in enumerate(ORDER):
        mark=name in ('u0301','u064e','u094d','u0e48')
        advance=0 if mark else 300 if name in ('u0020','u00a0','ka_half') else 900 if name=='fi' else 600
        width=100 if mark else 100+30*(i%8)+50*variant
        height=100 if mark else 400+30*(i%6)
        rects=[] if name in ('.notdef','u0020','u00a0') else [(0,0,width,height)]
        if name=='u0058':rects=[(-150,-150,250,400)]
        if name in (overrides or {}):advance,rects=overrides[name]
        pen=TTGlyphPen(None)
        for x0,y0,x1,y1 in rects:
            pen.moveTo((x0,y0));pen.lineTo((x0,y1));pen.lineTo((x1,y1));pen.lineTo((x1,y0));pen.closePath()
        glyphs[name]=pen.glyph();metrics[name]=(advance,min((r[0] for r in rects),default=0));geometry[name]=(advance,rects)
    builder.setupGlyf(glyphs);builder.setupHorizontalMetrics(metrics);builder.setupHorizontalHeader(ascent=900,descent=-200)
    builder.setupOS2(sTypoAscender=900,sTypoDescender=-200,usWinAscent=900,usWinDescent=200)
    builder.setupNameTable({'familyName':'Cutbolt Original Unicode Fixture','styleName':'Regular','uniqueFontIdentifier':path.stem,'fullName':path.stem,'psName':path.stem})
    builder.setupPost();builder.setupMaxp();addOpenTypeFeaturesFromString(builder.font,FEATURES+extra_features)
    builder.font['head'].created=builder.font['head'].modified=3800000000;builder.save(path)
    return geometry
