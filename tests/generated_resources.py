"""Compact generated sources: old-renderer equality and long caption planning."""
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import wave

from PIL import Image
from engine import ENGINE
from graphics import PRIMARY, make_font
from scenes import identity
from tracks import edit, time, track


def run(root, reference_engine=None):
    root=Path(root).resolve();sources=root/'sources';output=root/'output'
    sources.mkdir(parents=True);output.mkdir()
    reference_engine=Path(reference_engine or ENGINE)
    passed=[];frames_compared=0;samples_compared=0

    def call(command, engine=ENGINE, error=None, **fields):
        reply=subprocess.run([str(engine)],input=json.dumps(dict(command=command,**fields)).encode(),
            capture_output=True,timeout=180)
        value=json.loads(reply.stdout)
        if error:
            assert reply.returncode==1 and value['error']['code']==error,value
            return value['error']
        assert reply.returncode==0 and value['ok'],value
        return value['result']

    def base(name,width=24,height=16,duration=time(13,25),transparent=False,color=(19,73,151,255)):
        return dict(schema_version=1,id=name,width=width,height=height,output_scale=1,
            duration=duration,frame_rate=time(25),background=[11,22,33],color='srgb_straight_encoded',
            transparent=transparent,audio=None,layers=[dict(id='panel',canvas=[width,height],start=time(0),
                duration=duration,frames=[],graphics=dict(kind='shape',shape='rectangle',
                    rect=[0,0,width,height],fill=list(color),stroke=None),timing='strict',end='hold_last',
                transform=dict(position=[0,0],crop=[0,0,width,height],scale=1,quarter_turns=0,opacity=255))])

    def project(scene, name, asset=None, first=0, count=13, sound=True):
        value=call('project.create',id=name,width=scene['width'],height=scene['height'],frame_rate=time(25))
        v=track('v','video')
        if scene['transparent']:v['composite']='alpha_over'
        operations=[edit('create',duration=time(count,25)),edit('add',track=v)]
        if asset:operations.insert(0,dict(op='media.add',asset=asset))
        for name,kind in [('v','video')]+([('a','audio')] if sound else []):
            if kind=='audio':operations.append(edit('add',track=track(name,kind)))
            clip=dict(id=name,start=time(0),source_in=time(first,25),duration=time(count,25))
            clip.update({'asset_id':asset['id']} if asset else {'scene':scene})
            operations.append(edit('place',track_id=name,clip=clip,collision='reject'))
        return call('timeline.apply',project=value,expected_revision=0,operations=operations)

    def render(value,name):
        path=output/(name+'.mkv')
        call('render.run',project=value,input_root=str(sources),output_root=str(output),output=str(path))
        video=subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'],timeout=120)
        audio=subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(path),'-vn','-c:a','pcm_s16le','-f','s16le','-'],timeout=120)
        return video,audio

    zero=sources/'original-silence.wav'
    with wave.open(str(zero),'wb') as stream:
        stream.setparams((2,2,48000,13*1920,'NONE','not compressed'))
        stream.writeframes(bytes(13*1920*4))
    zero_bytes=zero.read_bytes()
    image=sources/'original-pattern.png';picture=Image.new('RGB',(24,16))
    picture.putdata([((x*17)%256,(y*23)%256,(x*11+y*7)%256) for y in range(16) for x in range(24)])
    picture.save(image)
    cases=[base('opaque'),base('blank',transparent=True,color=(0,0,0,0)),
        base('alpha',transparent=True,color=(19,73,151,128)),base('pattern'),base('recorded-silence')]
    layer=cases[3]['layers'][0];layer.pop('graphics')
    layer['frames']=[dict(image=identity(image,sources),hold=time(13,25),offset=[0,0],anchor=[0,0])]
    cases[4]['audio']=dict(file=identity(zero,sources),start=time(0),channels='preserve_stereo',resampling='linear',padding='silence')
    for scene in cases:
        name=scene['id']
        asset=call('scene.render',engine=reference_engine,scene=scene,input_root=str(sources),output_root=str(sources),
            output=str(sources/(name+'-reference.mkv')))['asset']
        old_pcm=subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(sources/(name+'-reference.mkv')),
            '-vn','-c:a','pcm_s16le','-f','s16le','-'],timeout=120)
        for first,count in [(0,13),(3,9)]:
            label=f'{name}-{first}'
            actual=render(project(scene,label,first=first,count=count),label+'-generated')
            # Legacy alpha assets are video-only overlay inputs. Their soundtrack
            # is compared directly below; the native scene still has an audio clip.
            expected=render(project(scene,label,asset,first,count,sound=not scene['transparent']),label+'-compiled')
            assert actual==expected,(label,'generated pixels or PCM differ from compiled source')
            assert actual[1]==old_pcm[first*1920*4:(first+count)*1920*4],label
            assert actual[1]==bytes(count*1920*4),label
            if name=='opaque':assert actual[0]==bytes([19,73,151])*(24*16*count)
            if name=='blank':assert actual[0]==bytes(24*16*3*count)
            if name=='pattern':assert actual[0]==picture.tobytes()*count
            frames_compared+=count;samples_compared+=count*1920
    passed.append('generated.compact_uniform_planes_dense_pixels_and_exact_silence')

    # A zero-valued soundtrack is still a pinned source, including on a cache hit.
    sound=project(cases[4],'sound-dependency')
    cache=root/'cache';cache.mkdir()
    task=dict(type='frame',project=sound,input_root=str(sources),output_root=str(output),time=time(0))
    policy=dict(max_bytes=64*1024*1024,max_entries=128)
    for n in range(2):
        value=call('cache.run',cache_root=str(cache),policy=policy,task={**task,'output':str(output/f'cached-{n}.png')})
        assert value['cache']['hit']==(n==1)
    zero.write_bytes(zero_bytes[:-1]+b'\1')
    denied=output/'changed.png'
    call('cache.run',error='MEDIA_CHANGED',cache_root=str(cache),policy=policy,task={**task,'output':str(denied)})
    assert not denied.exists();zero.write_bytes(zero_bytes)
    passed.append('generated.compact_silent_audio_keeps_pinned_dependencies')

    make_font(sources/'original.ttf',PRIMARY)
    scene=base('caption-window',1920,1080,time(10),True,(0,0,0,0))
    blank=scene['layers'][0]
    blank['canvas']=[1,1];blank['graphics']['rect']=[0,0,1,1];blank['transform']['crop']=[0,0,1,1]
    document=dict(schema_version=1,id='captions',revision=0,overlap='reject',styles={'default':{'color':[255,255,255]}},
        cues=[dict(id=f'cue-{n}',start=time(2*n),end=time(2*n+1),text='ABC ABC ABC',style='default',align='center',speaker=None) for n in range(5)])
    layouts={'default':dict(fonts=[identity(sources/'original.ttf',sources)],size=48,rect=[160,880,1600,180],
        line_height=60,letter_spacing=0,wrap='none',overflow='reject')}
    scene=call('captions.scene',document=document,scene=scene,scene_id='caption-window',offset=time(0),
        layouts=layouts,sampling='sample_start',layer_prefix='cue',input_root=str(sources))['scene']
    caption_project=call('project.create',id='long-captions',width=1920,height=1080,frame_rate=time(25))
    caption_track=track('captions','video');caption_track['composite']='alpha_over'
    operations=[edit('create',duration=time(1200)),edit('add',track=caption_track)]
    for n in range(120):
        recipe=copy.deepcopy(scene);recipe['id']=f'window-{n}'
        operations.append(edit('place',track_id='captions',collision='reject',clip=dict(id=f'caption-{n}',
            scene=recipe,start=time(n*10),source_in=time(0),duration=time(10))))
    caption_project=call('timeline.apply',project=caption_project,expected_revision=0,operations=operations)
    plan=call('render.plan',project=caption_project,input_root=str(sources),output_root=str(output),output=str(output/'planned.mkv'))
    resources=[c['scene_resources'] for c in plan['chunks']]
    assert len(resources)==2 and all(r==resources[0] for r in resources),resources
    assert resources[0]['scene_count']==120 and resources[0]['retained_payload_bytes']<128*1024*1024,resources[0]
    assert not (output/'planned.mkv').exists()
    passed.append('generated.twenty_minute_hd_caption_plan_has_bounded_retained_payload')
    assert zero.read_bytes()==zero_bytes and Image.open(image).tobytes()==picture.tobytes()
    report=dict(passed=passed,frames_compared=frames_compared,stereo_sample_frames_compared=samples_compared,
        reference_engine_sha256=hashlib.sha256(reference_engine.read_bytes()).hexdigest(),
        caption_plan=dict(seconds=1200,width=1920,height=1080,windows=120,chunks=len(resources),resources=resources[0]),
        source_preservation=True,long_caption_export_claimed=False)
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n')
    return report
