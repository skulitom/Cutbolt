"""Authored glyph plans, rational outline coverage and full scene/saved-edit acceptance."""
from engine import ENGINE, MCP_TOOLS
import budgets
import argparse,copy,hashlib,json,math,subprocess,time as clock
from fractions import Fraction as F
from pathlib import Path
from fontTools.ttLib import TTFont
from fontTools.ttLib.tables.DefaultTable import DefaultTable
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from compositing import expected_frame
from graphics import nearest,paint
from registry import peak_memory
from scenes import identity,time,audio_bytes
from templates import parameter,binding,value
from unicode_fonts import make_font,ORDER

ROOT=Path(__file__).resolve().parents[1]
EXE=ENGINE
def sha(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()
def g(name,cluster,font=0,advance=None,offset=(0,0)):
    return {'name':name,'cluster':cluster,'font':font,'advance':advance,'offset':offset}
def plain(text,start=0,font=0):
    result=[]
    for c in text:
        result.append(g(f'u{ord(c):04x}',start,font));start+=len(c.encode())
    return result

def raster_plan(graphic,canvas,plans,geometry):
    """Manual glyph order/placement plus analytic rectangle area; no shaping library."""
    image=Image.new('RGBA',tuple(canvas));x,y,w,h=graphic['rect'];size=graphic['size']
    expected=[];widths=[]
    for line,items in enumerate(plans):
        pen=0;previous=None;positions=[]
        for item in items:
            advance,rectangles=geometry[item['font']][item['name']]
            advance=advance if item['advance'] is None else item['advance']
            a=nearest(F(advance*size*64,1000));ox,oy=[nearest(F(v*size*64,1000)) for v in item['offset']]
            if previous is not None and previous!=item['cluster']:pen+=graphic['letter_spacing']*64
            positions.append((item,pen+ox,oy,a,rectangles));pen+=a;previous=item['cluster']
        widths.append(F(pen,64));shift={'left':0,'center':F(w*64-pen,2),'right':w*64-pen}[graphic['align']]
        baseline=y+size+line*graphic['line_height']
        for item,px64,py64,advance,rectangles in positions:
            origin=x+nearest(F(px64+shift,64));base=baseline-nearest(F(py64,64));coverage={}
            expected.append({'glyph_id':ORDER.index(item['name']),'font_index':item['font'],'cluster_utf8':item['cluster'],'line':line,'origin_x_128':int(x*128+2*(px64+shift)),'offset_y_64':py64,'advance_64':advance,'baseline':baseline})
            for l,b,r,t in rectangles:
                left,right=origin+F(l*size,1000),origin+F(r*size,1000)
                top,bottom=base-F(t*size,1000),base-F(b*size,1000)
                for yy in range(math.floor(top),math.ceil(bottom)):
                    for xx in range(math.floor(left),math.ceil(right)):
                        area=(min(right,xx+1)-max(left,xx))*(min(bottom,yy+1)-max(top,yy))
                        coverage[xx,yy]=coverage.get((xx,yy),F(0))+area
            for (xx,yy),area in coverage.items():
                if x<=xx<x+w and y<=yy<y+h:paint(image,xx,yy,graphic['color'],nearest(area*255))
    return image,expected,list(map(float,widths))

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,out,oracle,store=[root/n for n in ('sources','output','oracle','store')]
    for p in (sources,out,oracle,store):p.mkdir()
    geometry=[make_font(sources/'first.ttf',missing=(0x301,0x62a)),make_font(sources/'second.ttf',variant=1)]
    fonts=[identity(sources/name,sources) for name in ('first.ttf','second.ttf')]
    hashes={p:sha(p) for p in sources.iterdir()}
    passed=[];cases=[];decoded=0;previews=0;rejected=0;maximum_error=0
    def call(request,error=None,timeout=120):
        nonlocal rejected
        p=subprocess.run([str(EXE)],input=json.dumps(request).encode(),capture_output=True,timeout=timeout)
        try:r=json.loads(p.stdout)
        except Exception:raise AssertionError((p.returncode,p.stdout,p.stderr))
        if error:
            assert p.returncode==1 and r['error']['code']==error,(error,r);rejected+=1;return r
        if p.returncode or not r['ok']:(root/'failed-request.json').write_text(json.dumps(request,indent=2),encoding='utf-8')
        assert p.returncode==0 and r['ok'],r;return r['result']
    def text(content,**fields):
        return {'kind':'text','text':content,'fonts':copy.deepcopy(fonts),'size':20,'color':[211,73,149,173],'rect':[4,4,248,120],'line_height':24,'letter_spacing':0,'align':'left','wrap':'none','overflow':'reject','layout':{'profile':'unicode_v1','direction':'auto','language':'und'},**fields}
    def layer(graphic,name='title',frames=3):
        return {'id':name,'canvas':[256,128],'start':time(0),'duration':time(frames,25),'frames':[],'graphics':graphic,'timing':'strict','end':'hold_last','transform':{'position':[0,0],'crop':[0,0,256,128],'scale':1,'quarter_turns':0,'opacity':255}}
    def scene(layers,frames=3):
        return {'schema_version':1,'id':'unicode-fixture','width':256,'height':128,'output_scale':1,'duration':time(frames,25),'background':[17,53,119],'color':'srgb_straight_encoded','layers':layers,'audio':None}
    def inspect(s,error=None):return call({'command':'scene.inspect','scene':s,'input_root':str(sources)},error)
    def render(s,name,error=None):return call({'command':'scene.render','scene':s,'input_root':str(sources),'output_root':str(out),'output':str(out/f'{name}.mkv')},error)
    def compare(path,expected,size,tolerance=1):
        nonlocal decoded,maximum_error
        raw=subprocess.check_output(['ffmpeg','-v','error','-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'],timeout=90)
        assert len(raw)==len(expected)*size
        delta=max((abs(a-b) for a,b in zip(raw,b''.join(expected))),default=0)
        assert delta<=tolerance,(path.name,delta,tolerance)
        assert audio_bytes(path)==bytes(len(expected)*1920*4)
        decoded+=len(expected);maximum_error=max(maximum_error,delta);return delta
    def check(s,plans,name,tolerance=1):
        reference=copy.deepcopy(s);wanted=[]
        for i,(item,plan) in enumerate(zip(reference['layers'],plans)):
            graphic=item.pop('graphics');image,glyphs,widths=raster_plan(graphic,item['canvas'],plan,geometry)
            path=oracle/f'{name}-{i}.png';image.save(path);item['frames']=[{'image':identity(path,oracle),'hold':item['duration'],'offset':[0,0],'anchor':[0,0]}]
            wanted.append((glyphs,widths))
        info=inspect(s)
        for row,(glyphs,widths),source_layer in zip(info['timing'],wanted,s['layers']):
            layout=row['graphics']
            if source_layer['graphics'].get('layout'):
                assert layout['layout']=='unicode_v1' and len(layout['glyphs'])==len(glyphs)
                actual=[{k:v[k] for k in glyphs[i]} for i,v in enumerate(layout['glyphs'])] if glyphs else layout['glyphs']
                assert actual==glyphs,(name,actual,glyphs)
            else:
                assert layout['layout']=='ltr_scalar_v1' and [v['scalar'] for v in layout['glyphs']]==list(source_layer['graphics']['text'])
            assert layout['line_widths']==widths,(name,layout['line_widths'],widths)
        receipt=render(s,name);count=s['duration']['num']*25//s['duration']['den']
        expected=[expected_frame(reference,oracle,n) for n in range(count)]
        delta=compare(out/f'{name}.mkv',expected,s['width']*s['height']*s['output_scale']**2*3,tolerance)
        cases.append({'name':name,'frames':count,'maximum_rgb_delta':delta,'allowed_rgb_delta':tolerance,'glyphs':sum(len(g) for g,_ in wanted)})
        return info,receipt,expected,s

    # Every expected glyph plan is authored here from the original font tables.
    check(scene([layer(text('fi'))]),[[[g('fi',0)]]],'ligature')
    check(scene([layer(text('AB'))]),[[[g('u0041',0,advance=500),g('u0042',1)]]],'kerning')
    accent=[g('u0065',0,1),g('u0301',0,1,offset=(-350,700))]
    a=check(scene([layer(text('e\u0301'))]),[[accent]],'combining-fallback')
    b=check(scene([layer(text('\u00e9'))]),[[accent]],'canonical-decomposition')
    assert a[2]==b[2]
    arabic=[g('beh_final',4),g('teh_medial',2,1),g('beh_initial',0)]
    check(scene([layer(text('\u0628\u062a\u0628'))]),[[arabic]],'joining-across-fonts')
    check(scene([layer(text('\u0628\u064e\u0628'))]),[[[g('beh_final',4),g('u064e',0,offset=(250,700)),g('beh_initial',0)]]],'rtl-mark')
    check(scene([layer(text('\u0628\u200c\u0628'))]),[[[g('u0628',2),g('u0628',0)]]],'joining-break')
    check(scene([layer(text('\u0628\u200d\u0628'))]),[[[g('beh_final',5),g('beh_initial',0)]]],'joining-control')
    check(scene([layer(text('A\u0915\u093fB'))]),[[[g('u0041',0),g('u093f',1),g('u0915',1),g('u0042',7)]]],'mixed-scripts')
    check(scene([layer(text('\u0915\u094d\u0915\u093f'))]),[[[g('u093f',0),g('ka_half',0),g('u0915',0)]]],'indic-conjunct')
    check(scene([layer(text('\u0e01\u0e48'))]),[[[g('u0e01',0),g('u0e48',0,offset=(-350,700))]]],'thai-mark')
    check(scene([layer(text('\U00010400 \u4e2d'))]),[[plain('\U00010400 \u4e2d')]],'supplementary-cjk')
    check(scene([layer(text('\u0431',layout={'profile':'unicode_v1','language':'sr'}))]),[[[g('cyr_local',0)]]],'language-localization')
    check(scene([layer(text('\u0431'))]),[[plain('\u0431')]],'default-language')
    passed.append('unicode.original_substitutions_marks_scripts_and_fallback')

    mixed=plain('A (')+plain('12',10)+[g('u0020',9),g('u05d2',7),g('u05d1',5),g('u05d0',3)]+plain(') B',12)
    check(scene([layer(text('A (\u05d0\u05d1\u05d2 12) B'))]),[[mixed]],'mixed-bidi-numbers')
    check(scene([layer(text('\u05d0(\u05d1)\u05d2'))]),[[[g('u05d2',6),g('u0028',5),g('u05d1',3),g('u0029',2),g('u05d0',0)]]],'mirrored-brackets')
    # The removed PDI is retained in the following space's source cluster.
    check(scene([layer(text('A \u2067\u05d0\u05d1\u2069 B'))]),[[plain('A ')+[g('u05d1',7),g('u05d0',5),g('u0020',9),g('u0042',13)]]],'direction-isolate')
    check(scene([layer(text('A \u05d0\u05d1 B',layout={'profile':'unicode_v1','direction':'rtl'}))]),[[[g('u0042',7),g('u0020',6),g('u05d1',4),g('u05d0',2),g('u0020',1),g('u0041',0)]]],'explicit-rtl-paragraph')
    check(scene([layer(text('A \u05d0\u05d1 BB',rect=[4,4,24,120],wrap='character'))]),[[plain('A '),[g('u05d1',4),g('u05d0',2)],plain(' B',6),plain('B',8)]],'wrapped-paragraph-direction')
    passed.append('unicode.paragraph_bidi_controls_and_mirroring')

    wrapping=[[g('u0065',0,1),g('u0301',0,1,offset=(-350,700))],[g('u0065',3,1),g('u0301',3,1,offset=(-350,700))]]
    for align in ('left','center','right'):
        check(scene([layer(text('e\u0301e\u0301',rect=[4,4,23,120],wrap='character',align=align))]),[wrapping],'wrapped-accent-'+align)
    check(scene([layer(text('fi',rect=[4,4,12,120],wrap='character'))]),[[plain('f'),plain('i',1)]],'wrapped-ligature')
    check(scene([layer(text('\u0628\u0628\u0628',rect=[4,4,24,120],wrap='character'))]),[[[g('beh_final',2),g('beh_initial',0)],[g('u0628',4)]]],'joining-resets-at-line')
    check(scene([layer(text('A B A',rect=[4,4,35,120],wrap='word'))]),[[plain('A '),plain('B A',2)]],'word-wrapping')
    check(scene([layer(text('\u4e2d\u4e2d\u4e2d',rect=[4,4,24,120],wrap='word'))]),[[plain('\u4e2d\u4e2d'),plain('\u4e2d',6)]],'cjk-line-breaking')
    check(scene([layer(text('A\u200bB',rect=[4,4,12,120],wrap='word'))]),[[plain('A'),plain('B',4)]],'zero-width-break')
    check(scene([layer(text('\nA\n\n'))]),[[[],plain('A',1),[],[]]],'empty-lines')
    empty_rtl=check(scene([layer(text('\n\n',layout={'profile':'unicode_v1','direction':'rtl'}))]),[[[],[],[]]],'empty-rtl-lines')[0]
    assert all(line['base_direction']=='rtl' for line in empty_rtl['timing'][0]['graphics']['lines'])
    spaced=accent+[g('u0042',3)]
    check(scene([layer(text('e\u0301B',letter_spacing=3))]),[[spaced]],'cluster-spacing')
    check(scene([layer(text('\u0915\u093f\u0915\u093f',rect=[4,4,12,120],wrap='character',overflow='clip'))]),[[[g('u093f',0),g('u0915',0)],[g('u093f',6),g('u0915',6)]]],'oversized-graphemes')
    check(scene([layer(text('X',overflow='clip'))]),[[plain('X')]],'negative-bearing-clip')
    check(scene([layer(text('A',rect=[4,4,11,120],align='center',overflow='clip'))]),[[plain('A')]],'negative-half-origin')
    for size in (13,37):
        check(scene([layer(text('AB',size=size,align='center'))]),[[[g('u0041',0,advance=500),g('u0042',1)]]],f'fractional-metrics-{size}')
    check(scene([layer(text('e\u0301',color=[200,21,17,0]))]),[[accent]],'transparent-layout')
    for reverse in (False,True):
        old=layer(text('fi',layout=None),'old');new=layer(text('fi'),'new');new['transform']['position'][1]=24
        rows=[old,new];plans=[[plain('fi')],[[g('fi',0)]]]
        if reverse:rows.reverse();plans.reverse()
        check(scene(rows),plans,'mixed-profile-cache-'+str(reverse).lower())
    passed.append('unicode.wrap_alignment_spacing_and_transparency')

    # Overlapping marks/letters and existing animated masks/compositing use the same path.
    moving=scene([layer(text('e\u0301',color=[31,217,91,119]),'rear',8),layer(text('\u0628\u062a\u0628'),'front',8)],8)
    moving['output_scale']=2;moving['layers'][1]['blend_mode']='screen'
    moving['layers'][1]['animation']={'position_x':{'keys':[{'time':time(0),'value':0,'interpolation':'linear'},{'time':time(7,25),'value':25,'interpolation':'hold'}]},'opacity':{'keys':[{'time':time(0),'value':41,'interpolation':'linear'},{'time':time(7,25),'value':255,'interpolation':'hold'}]}}
    moving['layers'][1]['mask']={'rect':[0,0,48,128],'inverted':False}
    moving_info,moving_receipt,moving_frames,_=check(moving,[[accent],[arabic]],'animated-overlays',2)
    template={'schema_version':1,'id':'unicode-template','scene':scene([layer(text('AB'))]),'parameters':[parameter('label','text','AB',[binding('text','title')],max_chars=1024),parameter('fonts','fonts',fonts,[binding('fonts','title')])]}
    inst=call({'command':'graphics.instantiate','template':template,'values':{'label':value('text','e\u0301')},'instance_id':'unicode-template-instance','input_root':str(sources)})
    assert template['scene']['layers'][0]['graphics']['text']=='AB' and inst['scene']['layers'][0]['graphics']['layout']['profile']=='unicode_v1'
    check(inst['scene'],[[accent]],'template-typed-text')
    subtitle=sources/'original.srt';subtitle.write_text('1\n00:00:00,000 --> 00:00:00,080\ne\u0301\n\n2\n00:00:00,080 --> 00:00:00,160\n\u0628\u062a\u0628\n\n',encoding='utf-8');hashes[subtitle]=sha(subtitle)
    document=call({'command':'captions.import','id':'unicode-captions','format':'srt','source':identity(subtitle,sources),'input_root':str(sources),'overlap':'reject'})['document']
    request={'command':'captions.scene','document':document,'scene':scene([],4),'scene_id':'unicode-caption-scene','offset':time(0),'layouts':{'default':{'fonts':fonts,'size':20,'rect':[4,4,248,120],'line_height':24,'letter_spacing':0,'wrap':'word','overflow':'reject','text_layout':{'profile':'unicode_v1','direction':'auto','language':'und'}}},'sampling':'sample_start','layer_prefix':'caption','input_root':str(sources)}
    caption=call(request);check(caption['scene'],[[accent],[arabic]],'caption-scenes')
    passed.append('unicode.templates_captions_and_animated_scenes')

    client=Client(EXE)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==MCP_TOOLS
        for tool_name,args in [('cutbolt_scene_inspect',{'scene':moving,'input_root':str(sources)}),('cutbolt_captions_scene',{k:v for k,v in request.items() if k!='command'})]:
            tool=next(t for t in catalog if t['name']==tool_name);assert tool['annotations']['readOnlyHint'];Draft202012Validator(tool['inputSchema']).validate(args)
        assert client.call('scene.inspect',scene=moving,input_root=str(sources),detail='full')==moving_info
        assert client.call('captions.scene',detail='full',**{k:v for k,v in request.items() if k!='command'})==caption
        project=call({'command':'project.create','id':'unicode-edit','width':512,'height':256,'frame_rate':time(25)})
        client.call('session.create',store_root=str(store),project=project,request_id='create')
        edit={'store_root':str(store),'project_id':'unicode-edit','request_id':'place','expected_revision':0,'operations':[{'op':'media.add','asset':moving_receipt['asset']},{'op':'clip.append','clip':{'id':'title','asset_id':moving['id'],'source_in':time(1,25),'duration':time(6,25)}}]}
        changed=client.call('session.apply',**edit);assert client.call('session.apply',**edit)==changed
        saved=client.call('session.get',store_root=str(store),project_id='unicode-edit')
        call({'command':'render.run','project':saved,'input_root':str(out),'output_root':str(out),'output':str(out/'saved.mkv')});compare(out/'saved.mkv',moving_frames[1:7],512*256*3,2)
        for n in (0,3,5):
            path=out/f'preview-{n}.png';call({'command':'preview.frame','project':saved,'input_root':str(out),'output_root':str(out),'output':str(path),'time':time(n,25)})
            actual=Image.open(path).convert('RGB').tobytes();assert max(abs(a-b) for a,b in zip(actual,moving_frames[n+1]))<=2;previews+=1
        call({'command':'preview.range','project':saved,'input_root':str(out),'output_root':str(out),'output':str(out/'range.mkv'),'start':time(2,25),'duration':time(3,25)})
        compare(out/'range.mkv',moving_frames[3:6],512*256*3,2);previews+=1
        client.call('session.undo',store_root=str(store),project_id='unicode-edit',request_id='undo',expected_revision=1)
        client.call('session.restore',store_root=str(store),project_id='unicode-edit',request_id='restore',expected_revision=2,target_revision=1)
        assert len(client.call('session.history',store_root=str(store),project_id='unicode-edit')['entries'])==4
    finally:client.close()
    passed.append('unicode.mcp_saved_retries_undo_and_previews')

    # Bounded maximum text work and real process memory: all 1,024 visible glyphs laid out.
    maximum=scene([layer(text('A'*1024,size=1,line_height=2,wrap='character',overflow='clip',rect=[0,0,512,128]))],3)
    maximum['width']=512;maximum['layers'][0]['canvas']=[512,128];maximum['layers'][0]['transform']['crop']=[0,0,512,128]
    req={'command':'scene.inspect','scene':maximum,'input_root':str(sources)}
    started=clock.monotonic();process=subprocess.Popen([str(EXE)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    # Output is larger than the pipe buffer; drain it concurrently through communicate.
    import threading
    reply={}
    def collect():reply['stdout'],reply['stderr']=process.communicate(json.dumps(req).encode())
    reader=threading.Thread(target=collect);reader.start();peak=0
    while process.poll() is None:
        try:peak=max(peak,peak_memory(process.pid))
        except AssertionError:
            if process.poll() is None:raise
        clock.sleep(.01)
    reader.join(timeout=5);wall=clock.monotonic()-started;result=json.loads(reply['stdout']);assert process.returncode==0 and result['ok'],result
    report=result['result']['timing'][0]['graphics'];assert len(report['glyphs'])==1024 and report['shaped_input_scalars']<=1048576
    assert peak<256*1024*1024,(peak,wall)
    budgets.check(wall<30,(peak,wall))
    performance={'glyphs':1024,'peak_working_set_bytes':peak,'seconds':wall,'shaped_input_scalars':report['shaped_input_scalars'],'gates':{'maximum_bytes':256*1024*1024,'maximum_seconds':30}}
    per_line=512*64//nearest(F(600*64,1000))
    maximum_plans=[plain('A'*min(per_line,1024-i),i) for i in range(0,1024,per_line)]
    check(maximum,[maximum_plans],'maximum-text-full-render',2)
    check(scene([layer(text('A'*32,wrap='character',overflow='clip',rect=[0,0,12,128]))]),[[plain('A',i) for i in range(32)]],'long-clipped-lines')
    passed.append('unicode.bounded_work_and_font_metrics')

    invalid=[]
    def bad(fields,code='INVALID_GRAPHIC'):
        invalid.append((scene([layer(text('A',**fields))]),code))
    for tag in ('','en--GB','en_ GB','123','abcdefghi'):
        bad({'layout':{'profile':'unicode_v1','language':tag}})
    for contents in ('A\tB','A\rB','A\u2028B','A\u2029B','A\u00adB','A\ufe0f','A\U000e0100'):
        bad({'text':contents},'UNSUPPORTED_TEXT')
    bad({'text':'Z'},'MISSING_GLYPH');bad({'text':'e\u0301','fonts':[fonts[0]]},'MISSING_GLYPH')
    bad({'text':'X'},'TEXT_OVERFLOW');bad({'text':'\u0915\u093f','rect':[4,4,12,120],'wrap':'character'},'TEXT_OVERFLOW')
    bad({'text':'A\nA','rect':[4,4,30,22]},'TEXT_OVERFLOW')
    bad({'text':'A'*1025});bad({'layout':{'profile':'unknown'}},'INVALID_JSON');bad({'layout':{'profile':'unicode_v1','direction':'vertical'}},'INVALID_JSON')
    bad({'layout':{'profile':'unicode_v1','unknown':1}},'INVALID_JSON')
    bad({'layout':None,'wrap':'word'},'UNSUPPORTED_TEXT')
    bad({'fonts':[{**fonts[0],'sha256':'0'*64}]},'MEDIA_CHANGED')
    bad({'fonts':[fonts[0],{**fonts[0],'sha256':'0'*64}]},'INVALID_IDENTITY')
    bad({'fonts':[{**fonts[0],'bytes':8388609}]},'LIMIT_EXCEEDED')
    (sources/'invalid.ttf').write_bytes(b'\0\1\0\0'+bytes(12));hashes[sources/'invalid.ttf']=sha(sources/'invalid.ttf')
    bad({'fonts':[identity(sources/'invalid.ttf',sources)]},'UNSUPPORTED_FONT')
    for tag in ('COLR','fvar','morx'):
        font=TTFont(sources/'second.ttf');table=DefaultTable(tag);table.data=bytes(20);font[tag]=table
        path=sources/f'unsupported-{tag}.ttf';font.save(path);hashes[path]=sha(path)
        bad({'fonts':[fonts[0],identity(path,sources)]},'UNSUPPORTED_FONT')
    # Glyph metrics are bounded to 1024 pixels: 60000 units is far beyond it at size 128, and a
    # 5000-unit glyph exceeds it at size 256 (1280 pixels).
    for name,overrides,size in [('advance',{'u0041':(60000,[(0,0,100,100)])},128),('dimensions',{'u0041':(600,[(0,0,5000,5000)])},256)]:
        path=sources/f'{name}.ttf';make_font(path,overrides=overrides);hashes[path]=sha(path)
        bad({'fonts':[identity(path,sources)],'size':size,'overflow':'clip'},'UNSUPPORTED_FONT')
    path=sources/'expansion.ttf';make_font(path,extra_features='feature ccmp {sub u0041 by '+' '.join(['u0042']*16)+';} ccmp;');hashes[path]=sha(path)
    bad({'text':'A'*600,'fonts':[identity(path,sources)],'overflow':'clip'},'LIMIT_EXCEEDED')
    path=sources/'raster-work.ttf';make_font(path,overrides={'u0041':(4000,[(0,0,4000,4000)])});hashes[path]=sha(path)
    bad({'text':'A'*129,'fonts':[identity(path,sources)],'size':128,'overflow':'clip'},'LIMIT_EXCEEDED')
    for i,(s,code) in enumerate(invalid):render(s,f'invalid-{i}',code);assert not (out/f'invalid-{i}.mkv').exists()
    original_output=sha(out/'animated-overlays.mkv');render(moving,'animated-overlays','OUTPUT_EXISTS');assert sha(out/'animated-overlays.mkv')==original_output
    # Change only a generated font after scene preparation has completed, then restore it.
    mutation=scene([layer(text('AB'),frames=250)],250);mutation['width']=512;mutation['height']=256
    destination=out/'changed-font.mkv';request={'command':'scene.render','scene':mutation,'input_root':str(sources),'output_root':str(out),'output':str(destination)}
    process=subprocess.Popen([str(EXE)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    process.stdin.write(json.dumps(request).encode());process.stdin.close();original=(sources/'first.ttf').read_bytes();changed_font=False
    try:
        deadline=clock.monotonic()+15
        while process.poll() is None and clock.monotonic()<deadline:
            # Preparation is complete once frames stream into the scene's scratch encoder output.
            if list(out.glob('.cutbolt-scene-*/output.mkv')):
                (sources/'first.ttf').write_bytes(original+b'changed original fixture');changed_font=True;break
            clock.sleep(.002)
        assert changed_font,'Failed to inject the post-preparation font change'
        process.wait(timeout=90);result=json.loads(process.stdout.read());assert process.returncode==1 and result['error']['code']=='MEDIA_CHANGED',result
        assert not destination.exists();rejected+=1
    finally:
        (sources/'first.ttf').write_bytes(original)
        if process.poll() is None:process.kill();process.wait(timeout=5)
    assert not list(out.glob('.cutbolt-scene-*'))
    for path,digest in hashes.items():assert sha(path)==digest,path
    passed.append('unicode.validation_changed_fonts_and_output_preservation')
    result={'passed':passed,'decoded_frames':decoded,'silent_stereo_sample_frames':decoded*1920,'previews':previews,'rejected_cases':rejected,'maximum_rgb_delta':maximum_error,'cases':cases,'performance':performance,'font_identities':fonts,'source_preservation':True,'oracle':'Authored GSUB/GPOS rules and manual visual glyph plans, independent Fraction scaling/alignment and rectangle-area coverage; complete decoded pixels/PCM; shaping library is not used by the oracle'}
    (root/'scene.json').write_text(json.dumps(moving,indent=2),encoding='utf-8');(root/'verification.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
    print(json.dumps({k:v for k,v in result.items() if k not in ('cases','font_identities')}))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
