"""Original subtitle text, external demux timing, and independent caption pixel references."""
from engine import ENGINE
import argparse
import copy
from fractions import Fraction as F
import hashlib
import json
from pathlib import Path
import subprocess
from PIL import Image
from jsonschema import Draft202012Validator

from agents import Client
from compositing import expected_frame
from graphics import PRIMARY,FALLBACK,make_font,text_pixels
from scenes import decode_check,audio_bytes,identity

ROOT=Path(__file__).resolve().parents[1]
def time(n,d=1):
    f=F(n,d);return {"num":f.numerator,"den":f.denominator}
def rational(t):return F(t["num"],t["den"])


def packets(path):
    result=json.loads(subprocess.check_output(['ffprobe','-v','error','-show_packets','-show_data','-of','json',str(path)],timeout=40))
    values=[]
    for p in result['packets']:
        data=b''.join(bytes.fromhex(line.split(': ',1)[1][:39]) for line in p['data'].splitlines() if line)
        values.append((F(p['pts_time']),F(p['duration_time']),data.decode('utf-8')))
    return values


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,output,oracle,store=[root/n for n in ('sources','output','oracle','store')]
    for directory in (sources,output,oracle,store):directory.mkdir()
    srt='\ufeff1\r\n00:00:00,000 --> 00:00:00,041\r\nAB\r\nC\r\n\r\n2\r\n00:00:00,040 --> 00:00:00,160\r\nΩA\r\n\r\n3\r\n00:00:00,160 --> 00:00:00,240\r\n中\r\n\r\n4\r\n00:00:00,161 --> 00:00:00,162\r\nB\r\n\r\n5\r\n00:00:02,000 --> 00:00:02,040\r\nA\r\n\r\n'
    vtt='WEBVTT\n\nSTYLE\n::cue(.warm) { color: #ed6320; }\n::cue(.cool) { color: #20a4dd; }\n\nalpha\n00:00.000 --> 00:00.081 align:left\n<v Guide><c.warm>AB\nC</c></v>\n\nbeta\n00:00.040 --> 00:00.200 align:right\n<c.cool>Ω中</c>\n\nshort\n00:00.081 --> 00:00.082\n<c.warm>B</c>\n\nescaped\n00:00.200 --> 00:00.280\n<c.cool>A &amp; B &lt;C&gt;</c>\n\nhour\n00:59:59.999 --> 01:00:00.001 align:left\n<c.warm>A</c>\n\n'
    (sources/'original.srt').write_text(srt,encoding='utf-8',newline='')
    (sources/'original.vtt').write_text(vtt,encoding='utf-8')
    (sources/'unicode.srt').write_text('1\n00:00:01,001 --> 00:00:02,003\nCafé مرحبا 😀\n\n',encoding='utf-8')
    make_font(sources/'primary.ttf',PRIMARY);make_font(sources/'fallback.ttf',FALLBACK)
    fonts=[identity(sources/n,sources) for n in ('primary.ttf','fallback.ttf')]
    font_data={'primary.ttf':PRIMARY,'fallback.ttf':FALLBACK}
    backdrop=Image.new('RGB',(96,64))
    for y in range(64):
        for x in range(96):backdrop.putpixel((x,y),(20+x,30+y,50+(x+y)%100))
    backdrop.save(sources/'backdrop.png')
    hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ENGINE;passed=[];decoded=0
    def request(cmd,error=None):
        p=subprocess.run([str(exe)],input=json.dumps(cmd).encode(),capture_output=True,timeout=120)
        reply=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and reply['error']['code']==error,(error,reply)
            return reply
        assert p.returncode==0 and reply['ok'],reply
        return reply['result']
    def import_file(name,fmt,overlap='allow',error=None):
        return request({'command':'captions.import','id':'captions','format':fmt,'source':identity(sources/name,sources),'input_root':str(sources),'overlap':overlap},error)
    def inspect(doc,error=None):return request({'command':'captions.inspect','document':doc},error)
    def encode(doc,fmt,error=None):return request({'command':'captions.encode','document':doc,'format':fmt},error)
    def export(doc,fmt,name,policy='reject',error=None):
        return request({'command':'captions.export','document':doc,'format':fmt,'loss_policy':policy,'output_root':str(output),'output':str(output/name)},error)
    def apply(doc,ops,error=None):return request({'command':'captions.apply','document':doc,'expected_revision':doc['revision'],'operations':ops},error)
    cap=request({'command':'capabilities','section':'all'})
    assert cap['captions']['export_time']=='exact_milliseconds_no_rounding'
    simple=import_file('original.srt','srt')['document']
    expected_intervals=[(0,41,'AB\nC'),(40,160,'ΩA'),(160,240,'中'),(161,162,'B'),(2000,2040,'A')]
    assert [(rational(c['start'])*1000,rational(c['end'])*1000,c['text']) for c in simple['cues']]==expected_intervals
    assert inspect(simple)['maximum_simultaneous']==2
    import_file('original.srt','srt','reject','CAPTION_OVERLAP')
    export(simple,'srt','roundtrip.srt')
    assert packets(output/'roundtrip.srt')==[(F(a,1000),F(b-a,1000),text) for a,b,text in expected_intervals]
    imported=request({'command':'captions.import','id':'captions','format':'srt','source':identity(output/'roundtrip.srt',output),'input_root':str(output),'overlap':'allow'})['document']
    assert imported==simple
    unicode=import_file('unicode.srt','srt')['document'];export(unicode,'srt','unicode.srt')
    assert packets(output/'unicode.srt')==[(F(1001,1000),F(1002,1000),'Café مرحبا 😀')]
    styled=import_file('original.vtt','webvtt')['document']
    assert styled['styles']=={'warm':{'color':[237,99,32]},'cool':{'color':[32,164,221]}}
    assert styled['cues'][0]['text']=='AB\nC' and styled['cues'][0]['speaker']=='Guide'
    assert styled['cues'][1]['align']=='right' and styled['cues'][3]['text']=='A & B <C>'
    assert rational(styled['cues'][-1]['end'])-rational(styled['cues'][-1]['start'])==F(1,500)
    receipt=export(styled,'webvtt','styled.vtt')
    roundtrip=request({'command':'captions.import','id':'captions','format':'webvtt','source':identity(output/'styled.vtt',output),'input_root':str(output),'overlap':'allow'})['document']
    assert roundtrip==styled and not receipt['losses']
    external=packets(output/'styled.vtt')
    assert [(p[0],p[1]) for p in external]==[(rational(c['start']),rational(c['end'])-rational(c['start'])) for c in styled['cues']]
    assert external[0][2]=='<v Guide><c.warm>AB\nC</c></v>' and '&amp;' in external[3][2]
    passed.append('captions.import_export_exact_interchange')

    # Required loss decisions are inspectable before writing; the document remains unchanged.
    portable=copy.deepcopy(styled);portable['cues'][3]['text']='A B C'
    plan=encode(portable,'srt')
    assert plan['losses'][0]=={'style_ids':['cool','warm'],'fields':['style_definitions']}
    assert plan['losses'][1]=={'cue_id':'alpha','fields':['cue_id','style','alignment','speaker']}
    export(portable,'srt','rejected.srt',error='LOSSY_CAPTION_EXPORT');assert not (output/'rejected.srt').exists()
    exported=export(portable,'srt','reported.srt','allow_reported')
    assert exported['losses']==plan['losses'] and exported['cue_id_map'][0]=={'cue_id':'alpha','exported_id':'1'}
    assert packets(output/'reported.srt')==[(rational(c['start']),rational(c['end'])-rational(c['start']),c['text']) for c in portable['cues']]
    # Built-in standard color classes and cue IDs generated without colliding with explicit IDs.
    (sources/'classes.vtt').write_text('WEBVTT\n\n00:00.000 --> 00:00.080\n<c.yellow>A</c>\n\ncue-1\n00:00.080 --> 00:00.160\nB\n\n',encoding='utf-8')
    classes=import_file('classes.vtt','webvtt')['document']
    assert classes['cues'][0]['id']=='cue-2' and classes['styles']['yellow']['color']==[255,255,0]
    assert inspect(classes)['maximum_simultaneous']==1
    (sources/'default-class.vtt').write_text('WEBVTT\n\nSTYLE\n::cue(.default) { color: #123456; }\n\n00:00.000 --> 00:00.040\nA\n\n00:00.040 --> 00:00.080\n<c.default>B</c>\n\n',encoding='utf-8')
    default_class=import_file('default-class.vtt','webvtt')['document']
    assert default_class['styles'][default_class['cues'][0]['style']]['color']==[255,255,255]
    assert default_class['styles'][default_class['cues'][1]['style']]['color']==[18,52,86]
    export(default_class,'webvtt','default-class.vtt')
    assert request({'command':'captions.import','id':'captions','format':'webvtt','source':identity(output/'default-class.vtt',output),'input_root':str(output),'overlap':'allow'})['document']==default_class
    passed.append('captions.styles_voices_losses_unicode')

    original=copy.deepcopy(simple)
    changed=copy.deepcopy(simple['cues'][0]);changed.update(text='BC',style='warm',align='left')
    ops=[{'op':'style.set','style_id':'warm','style':{'color':[237,99,32]}},{'op':'cue.replace','cue':changed},
         {'op':'cues.shift','cue_ids':['1','2'],'offset':time(2,25),'backward':False},{'op':'cue.remove','cue_id':'4'}]
    edit=apply(simple,ops);assert edit['document']['revision']==1 and simple==original
    edited=edit['document'];cue1=next(c for c in edited['cues'] if c['id']=='1')
    assert cue1['start']==time(2,25) and cue1['end']==time(121,1000) and cue1['text']=='BC'
    assert {c['cue_id'] for c in edit['changes']}=={'1','2','4'}
    assert apply(simple,ops)==edit
    request({'command':'captions.apply','document':edited,'expected_revision':0,'operations':ops},'REVISION_CONFLICT')
    shifted=apply(edited,[{'op':'cues.shift','cue_ids':['1','2'],'offset':time(2,25),'backward':True}])['document']
    assert shifted['cues'][0]['start']==time(0)
    # Atomic final-state validation allows removing an old style and replacing every reference in one batch.
    replacements=[{'op':'cue.replace','cue':{**c,'style':'warm'}} for c in edited['cues']]
    reassigned=apply(edited,[{'op':'style.remove','style_id':'default'},*replacements])['document']
    assert set(reassigned['styles'])=={'warm'}
    extra={**changed,'id':'new','start':time(3),'end':time(4)}
    appended=apply(edited,[{'op':'cue.add','cue':extra}])['document'];assert appended['cues'][-1]==extra
    apply(simple,[{'op':'cue.add','cue':{**extra,'style':'default'}},{'op':'cue.remove','cue_id':'missing'}],'INVALID_CAPTIONS')
    assert inspect(simple)==inspect(original)
    passed.append('captions.atomic_edit_diffs_and_timing')

    layouts={name:{'fonts':copy.deepcopy(fonts),'size':20,'rect':rect,'line_height':24,'letter_spacing':0,'wrap':'none','overflow':'reject'} for name,rect in [('warm',[2,1,42,60]),('cool',[42,20,52,43])]}
    base={'schema_version':1,'id':'base','width':96,'height':64,'output_scale':2,'duration':time(10,25),'background':[1,2,3],'color':'srgb_straight_encoded',
          'layers':[{'id':'backdrop','canvas':[96,64],'start':time(0),'duration':time(10,25),'frames':[{'image':identity(sources/'backdrop.png',sources),'hold':time(10,25),'offset':[0,0],'anchor':[0,0]}],
                     'timing':'strict','end':'hold_last','transform':{'position':[0,0],'crop':[0,0,96,64],'scale':1,'quarter_turns':0,'opacity':255}}],'audio':None}
    (oracle/'backdrop.png').write_bytes((sources/'backdrop.png').read_bytes())
    render_doc=copy.deepcopy(portable);render_doc['cues'][3]['text']='AB'
    def scene_request(doc=render_doc,offset=time(0),name='captions-render'):
        return {'command':'captions.scene','document':doc,'scene':base,'scene_id':name,'offset':offset,'layouts':layouts,'sampling':'sample_start','layer_prefix':'caption','input_root':str(sources)}
    def expected(doc,offset,name):
        layers=[]
        for c in doc['cues']:
            layout=layouts[c['style']];graphic={**layout,'kind':'text','text':c['text'],'color':[*doc['styles'][c['style']]['color'],255],'align':c['align']}
            pixels,_,_=text_pixels(graphic,[96,64],font_data);path=oracle/(name+'-'+c['id']+'.png');pixels.save(path)
            layers.append((c,{'id':c['id'],'canvas':[96,64],'start':time(0),'duration':base['duration'],'frames':[{'image':identity(path,oracle),'hold':base['duration'],'offset':[0,0],'anchor':[0,0]}],
                             'timing':'strict','end':'hold_last','transform':{'position':[0,0],'crop':[0,0,96,64],'scale':1,'quarter_turns':0,'opacity':255}}))
        def frame(n):
            clock=rational(offset)+F(n,25)
            ref={**base,'layers':base['layers']+[l for c,l in layers if rational(c['start'])<=clock<rational(c['end'])]}
            return expected_frame(ref,oracle,n)
        return frame
    scene_cases=[]
    for name,offset in [('zero',time(0)),('between',time(1,1000)),('cut',time(1,25)),('hour',time(89999,25))]:
        info=request(scene_request(offset=offset,name=name));ref=expected(render_doc,offset,name)
        reports={r['cue_id']:r for r in info['cues']}
        for c in render_doc['cues']:
            selected=[n for n in range(10) if rational(c['start'])<=rational(offset)+F(n,25)<rational(c['end'])]
            if selected:
                assert reports[c['id']]['first_frame']==min(selected) and reports[c['id']]['end_frame']==max(selected)+1
            else:assert reports[c['id']]['status'] in ('no_sampled_frame','outside_window')
        render=request({'command':'scene.render','scene':info['scene'],'input_root':str(sources),'output_root':str(output),'output':str(output/(name+'.mkv'))})
        decode_check(output/(name+'.mkv'),192,128,ref,10);assert audio_bytes(output/(name+'.mkv'))==b'\0'*(10*1920*4);decoded+=10
        scene_cases.append((name,info,render,ref))
    assert scene_cases[0][1]['cues'][2]['status']=='no_sampled_frame'
    # A caption window on a 29.97 fps scene samples cues on that clock.
    rate=F(30000,1001);base30=copy.deepcopy(base);base30.update(frame_rate={'num':30000,'den':1001},duration={'num':10*1001,'den':30000})
    base30['layers'][0].update(duration=base30['duration']);base30['layers'][0]['frames'][0]['hold']=base30['duration']
    native=request({**scene_request(name='captions-native'),'scene':base30})
    reports={r['cue_id']:r for r in native['cues']}
    for c in render_doc['cues']:
        selected=[n for n in range(10) if rational(c['start'])<=F(n)/rate<rational(c['end'])]
        if selected:assert reports[c['id']]['first_frame']==min(selected) and reports[c['id']]['end_frame']==max(selected)+1,(c['id'],reports[c['id']])
    compiled=request({'command':'scene.render','scene':native['scene'],'input_root':str(sources),'output_root':str(output),'output':str(output/'captions-native.mkv')})
    assert compiled['frames']==10 and compiled['samples']==16016 and compiled['frame_rate']=={'num':30000,'den':1001}
    assert scene_cases[0][3](0)!=scene_cases[0][3](1) and scene_cases[0][3](2)!=scene_cases[0][3](3)
    passed.append('captions.sampled_multiline_overlap_pixels')
    # A long caption track as one transparent overlay. Every frame is either empty or exactly the
    # independently rasterized text of the one cue active at its start time.
    words=['AB','C','ΩA','中']
    track={'schema_version':1,'id':'track','revision':0,'overlap':'reject','styles':{'warm':{'color':[237,99,32]},'cool':{'color':[32,164,221]}},
           'cues':[{'id':f'q{i}','start':time(4*i,10),'end':time(4*i+3,10),'text':words[i%4],'style':('warm','cool')[i%2],'align':('left','center','right')[i%3],'speaker':None} for i in range(20)]
                  +[{'id':'late','start':time(12),'end':time(1304,100),'text':'AB\nC','style':'warm','align':'left','speaker':None},
                    {'id':'seam','start':time(51,2),'end':time(53,2),'text':'ΩA','style':'cool','align':'right','speaker':None}]}
    rasters={}
    def overlay_frames(doc,rate,start,count):
        frames=[]
        for n in range(count):
            t=rational(start)+F(n)/rate
            active=[c for c in doc['cues'] if rational(c['start'])<=t<rational(c['end'])]
            assert len(active)<=1
            if not active:frames.append(bytes(96*64*4));continue
            c=active[0]
            if c['id'] not in rasters:
                graphic={**layouts[c['style']],'kind':'text','text':c['text'],'color':[*doc['styles'][c['style']]['color'],255],'align':c['align']}
                rasters[c['id']]=text_pixels(graphic,[96,64],font_data)[0].tobytes()
            frames.append(rasters[c['id']])
        return frames
    def overlay(project,name,rate,start=time(0),**fields):
        result=request({'command':'captions.render','document':track,'layouts':layouts,'project':project,'input_root':str(sources),
                        'output_root':str(output),'output':str(output/name),**({'start':start} if start!=time(0) else {}),**fields})
        decoded=subprocess.run(['ffmpeg','-v','error','-i',str(output/name),'-f','rawvideo','-pix_fmt','rgba','-'],capture_output=True,check=True).stdout
        count=result['frames'];assert len(decoded)==count*96*64*4
        expected=overlay_frames(track,rate,start,count)
        for n in range(count):
            assert decoded[n*96*64*4:(n+1)*96*64*4]==expected[n],(name,n)
        samples=int(F(count)/rate*48000)
        assert audio_bytes(output/name)==b'\0'*(samples*4)
        assert result['asset']['identity']['bytes']==(output/name).stat().st_size and result['asset']['duration']==time(F(count)/rate)
        return result
    canvas={'schema_version':1,'id':'overlay-project','revision':0,'width':96,'height':64,'frame_rate':time(25),'assets':[],'clips':[]}
    def timeline(duration,rate=time(25)):
        p=copy.deepcopy(canvas);p['frame_rate']=rate
        p['clips']=[{'id':'g','gap':True,'source_in':time(0),'duration':duration}]
        return p
    whole=overlay(timeline(time(30)),'track.mkv',F(25))
    # The first window ends before its 16th cue starts (6 s); the next windows run ten seconds.
    assert whole['frames']==750 and whole['windows']==4 and whole['covers']==time(30) and whole['cues']['no_sampled_frame']==[]
    part=overlay(timeline(time(30)),'track-part.mkv',F(25),start=time(24),duration=time(3))
    assert part['frames']==75 and part['windows']==1
    ntsc=overlay(timeline(time(301*1001,30000),time(30000,1001)),'track-ntsc.mkv',F(30000,1001))
    # Windows are whole multiples of 30 frames at 29.97 fps, so the asset runs to frame 330.
    assert ntsc['frames']==330 and ntsc['covers']==time(301*1001,30000)
    request({'command':'captions.render','document':track,'layouts':layouts,'project':timeline(time(30)),'input_root':str(sources),
             'output_root':str(output),'output':str(output/'track.mkv')},'OUTPUT_EXISTS')
    request({'command':'captions.render','document':track,'layouts':{'warm':layouts['warm']},'project':timeline(time(30)),'input_root':str(sources),
             'output_root':str(output),'output':str(output/'track-missing.mkv')},'INVALID_CAPTIONS')
    crowded={**track,'cues':[{'id':f'z{i}','start':time(i,1000),'end':time(i+1,1000),'text':'A','style':'warm','align':'left','speaker':None} for i in range(16)]}
    request({'command':'captions.render','document':crowded,'layouts':layouts,'project':timeline(time(1)),'input_root':str(sources),
             'output_root':str(output),'output':str(output/'crowded.mkv')},'LIMIT_EXCEEDED')
    assert not (output/'track-missing.mkv').exists() and not (output/'crowded.mkv').exists()
    assert not [p for p in output.iterdir() if p.name.startswith('.cutbolt')]
    passed.append('captions.long_track_overlay_pixels')

    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools']
        tool=next(x for x in catalog if x['name']=='cutbolt_captions_scene')
        args=scene_request(name='zero');args.pop('command');Draft202012Validator(tool['inputSchema']).validate(args)
        assert tool['annotations']['readOnlyHint'] and client.call('captions.scene',**args)==scene_cases[0][1]
        assert client.call('captions.inspect',document=simple)==inspect(simple)
        assert client.call('captions.import',id='captions',format='srt',source=identity(sources/'original.srt',sources),input_root=str(sources),overlap='allow')['document']==simple
        assert client.call('captions.apply',document=simple,expected_revision=0,operations=ops)==edit
        assert client.call('captions.encode',document=portable,format='srt')==plan
        export_tool=next(x for x in catalog if x['name']=='cutbolt_captions_export')
        assert not export_tool['annotations']['readOnlyHint'] and not export_tool['annotations']['idempotentHint']
        client.call('captions.export',document=simple,format='srt',loss_policy='reject',output_root=str(output),output=str(output/'mcp.srt'))
        assert (output/'mcp.srt').read_bytes()==(output/'roundtrip.srt').read_bytes()
        project=request({'command':'project.create','id':'caption-edit','width':192,'height':128,'frame_rate':time(25)})
        client.call('session.create',store_root=str(store),project=project,request_id='create')
        asset=scene_cases[0][2]['asset'];ops=[{'op':'media.add','asset':asset},{'op':'clip.append','clip':{'id':'captions','asset_id':asset['id'],'source_in':time(1,25),'duration':time(7,25)}}]
        receipt=client.call('session.apply',store_root=str(store),project_id='caption-edit',request_id='caption-asset',expected_revision=0,operations=ops)
        assert receipt==client.call('session.apply',store_root=str(store),project_id='caption-edit',request_id='caption-asset',expected_revision=0,operations=ops)
        saved=client.call('session.get',store_root=str(store),project_id='caption-edit')
        request({'command':'render.run','project':saved,'input_root':str(output),'output_root':str(output),'output':str(output/'saved.mkv')})
        decode_check(output/'saved.mkv',192,128,lambda n:scene_cases[0][3](n+1),7);assert audio_bytes(output/'saved.mkv')==b'\0'*(7*1920*4);decoded+=7
    finally:client.close()
    passed.append('captions.mcp_and_saved_session_use')

    invalid=[]
    def bad(change,code='INVALID_CAPTIONS'):
        d=copy.deepcopy(simple);change(d);invalid.append(({'command':'captions.inspect','document':d},code))
    bad(lambda d:d.update(schema_version=2));bad(lambda d:d['cues'].append(copy.deepcopy(d['cues'][0])))
    bad(lambda d:d['cues'][0].update(end=time(0)));bad(lambda d:d['cues'][0].update(text='A\n\nB'))
    bad(lambda d:d['cues'][0].update(text=''));bad(lambda d:d['cues'][0].update(text='A\tB'))
    bad(lambda d:d['cues'][0].update(text='A'*1025));bad(lambda d:d['cues'][0].update(style='missing'))
    bad(lambda d:d['cues'][0].update(id='STYLE'));bad(lambda d:d['cues'][0].update(speaker='A\nB'))
    bad(lambda d:d['cues'][0].update(start={'num':1,'den':0}),'INVALID_TIME')
    bad(lambda d:d['cues'][0].update(end=time(86401)));bad(lambda d:d['cues'].reverse())
    bad(lambda d:d.update(overlap='reject'),'CAPTION_OVERLAP')
    bad(lambda d:d.update(styles={str(i):{'color':[1,2,3]} for i in range(33)}))
    fractional=copy.deepcopy(simple);fractional['cues'][0]['start']=time(1,30000)
    invalid.append(({'command':'captions.encode','document':fractional,'format':'webvtt'},'UNALIGNED_TIME'))
    invalid += [({'command':'captions.apply','document':simple,'expected_revision':0,'operations':ops},code) for ops,code in [
        ([{'op':'style.remove','style_id':'default'}],'INVALID_CAPTIONS'),
        ([{'op':'cues.shift','cue_ids':['1'],'offset':time(1),'backward':True}],'INVALID_RANGE'),
        ([{'op':'cues.shift','cue_ids':['1','1'],'offset':time(0),'backward':False}],'INVALID_CAPTIONS'),
        ([{'op':'cue.replace','cue':extra}],'INVALID_CAPTIONS'),([],'INVALID_CAPTIONS')]]
    for change,code in [(lambda c:c['layouts'].pop('warm'),'INVALID_CAPTIONS'),
                        (lambda c:c['layouts']['warm'].update(rect=[0,0,1,1]),'TEXT_OVERFLOW'),
                        (lambda c:c['layouts']['warm']['fonts'][0].update(sha256='0'*64),'MEDIA_CHANGED'),
                        (lambda c:c.update(layer_prefix='backdrop',scene={**base,'layers':[{**base['layers'][0],'id':'backdrop-alpha'}]}),'INVALID_CAPTIONS'),
                        (lambda c:c.update(sampling='nearest'),'INVALID_JSON'),
                        (lambda c:c['document'].update(cues=[{**render_doc['cues'][0],'id':f'cue{i}'} for i in range(17)]),'INVALID_SCENE')]:
        cmd=copy.deepcopy(scene_request());change(cmd);invalid.append((cmd,code))
    bad_sources=[('WEBVTT\n\nalpha\n00:00.000 --> 00:00.040 position:20%\nA\n','webvtt','UNSUPPORTED_CAPTIONS'),
        ('WEBVTT\n\n00:00.000 --> 00:00.040\n<b>A</b>\n','webvtt','UNSUPPORTED_CAPTIONS'),
        ('WEBVTT\n\nSTYLE\n::cue(.x) { font-size: 20px; }\n','webvtt','UNSUPPORTED_CAPTIONS'),
        ('WEBVTT\n\n00:00.000 --> 00:00.040\n&unknown;\n','webvtt','UNSUPPORTED_CAPTIONS'),
        ('WEBVTT\n\n00:00.000 --> 00:00.040\n<c.red>A\n','webvtt','INVALID_CAPTIONS'),
        ('WEBVTT\n\n00:60.000 --> 00:61.000\nA\n','webvtt','INVALID_CAPTIONS'),
        ('WEBVTT\n\n00:00.0 --> 00:00.040\nA\n','webvtt','INVALID_CAPTIONS'),
        ('1\n00:00:00,040 --> 00:00:00,000\nA\n','srt','INVALID_CAPTIONS'),
        ('0\n00:00:00,000 --> 00:00:00,040\nA\n','srt','INVALID_CAPTIONS'),
        ('1\n00:00:00,000 --> 00:00:00,040\n<i>A</i>\n','srt','UNSUPPORTED_CAPTIONS')]
    for i,(text,fmt,code) in enumerate(bad_sources):
        name=f'invalid-{i}.txt';(sources/name).write_text(text,encoding='utf-8')
        invalid.append(({'command':'captions.import','id':'invalid','format':fmt,'source':identity(sources/name,sources),'input_root':str(sources),'overlap':'allow'},code))
    (sources/'invalid-utf8.srt').write_bytes(b'\xff\xfe\x00')
    invalid.append(({'command':'captions.import','id':'invalid','format':'srt','source':identity(sources/'invalid-utf8.srt',sources),'input_root':str(sources),'overlap':'allow'},'UNSUPPORTED_CAPTIONS'))
    invalid.append(({'command':'captions.import','id':'invalid','format':'srt','source':{**identity(sources/'original.srt',sources),'sha256':'0'*64},'input_root':str(sources),'overlap':'allow'},'MEDIA_CHANGED'))
    invalid.append(({'command':'captions.import','id':'invalid','format':'srt','source':{**identity(sources/'original.srt',sources),'bytes':2097153},'input_root':str(sources),'overlap':'allow'},'LIMIT_EXCEEDED'))
    before_outputs={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    for cmd,code in invalid:request(cmd,code)
    export(simple,'srt','roundtrip.srt',error='OUTPUT_EXISTS')
    request({'command':'captions.export','document':simple,'format':'srt','loss_policy':'reject','output_root':str(sources),'output':str(sources/'original.srt')},'OUTPUT_EXISTS')
    request({'command':'captions.export','document':simple,'format':'srt','loss_policy':'reject','output_root':str(output),'output':str(root/'escape.srt')},'PATH_OUTSIDE_ROOT')
    assert hashes=={name:hashlib.sha256((sources/name).read_bytes()).hexdigest() for name in hashes}
    assert before_outputs=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    assert not list(output.glob('.cutbolt-scene-*'))
    passed.append('captions.validation_and_source_output_preservation')
    report={'passed':passed,'decoded_frames':decoded,'silent_stereo_sample_frames':decoded*1920,'rejected_cases':len(invalid)+3,
            'external_subtitle_checks':'FFprobe 7.0 packet text and exact PTS/duration across SRT/WebVTT exports, Unicode and reported-loss export',
            'pixel_oracle':'Independent Fraction cue activity at frame starts, original glyph rectangle geometry and forward image composition; every RGB frame and silent PCM sample compared exactly',
            'scope':'UTF-8 SRT plain-text and declared WebVTT color-class/voice/alignment subset; immutable edits, exact rational times, explicit loss policy and bounded scene compilation with font/layout maps'}
    (root/'document.json').write_text(json.dumps(render_doc,indent=2)+'\n',encoding='utf-8')
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(report))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
