"""Content-bound native application captures, independent pixels and loss guards.

Native files and application-specific adapters remain in a reviewed external
fixture. This replays captured public-interface evidence, not a fresh app launch.
"""
from engine import ENGINE, MCP_TOOLS
import argparse
import copy
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

from agents import Client
from jsonschema import Draft202012Validator
from scenes import time
from tracks import move

ROOT=Path(__file__).resolve().parents[1]


def run(root, fixture):
    root=root.resolve();fixture=fixture.resolve()
    assert ROOT not in root.parents and root!=ROOT and ROOT not in fixture.parents
    root.mkdir(parents=True,exist_ok=True)
    sources=root/'sources';output=root/'output';store=root/'store'
    for path in (sources,output,store):path.mkdir()
    manifest=json.loads(fixture.read_text(encoding='utf-8'))
    assert manifest['schema_version']==1 and manifest['profile']=='native-flat-timeline-v1'
    assert manifest['capture_method']=='installed_application_public_interface'
    exe=ENGINE;passed=[];frames=samples=rejected=0
    def sha(path):
        with Path(path).open('rb') as stream:return hashlib.file_digest(stream,'sha256').hexdigest()
    def checked(item):
        path=Path(item['path']).resolve()
        assert path.is_absolute() and ROOT not in path.parents and path.is_file()
        assert path.stat().st_size==item['bytes'] and sha(path)==item['sha256'],path
        return path
    # Exact installed build and both original external adapter components remain
    # content-bound. No application binaries, native files or captures are copied
    # into the repository, and the engine never executes the external adapter.
    originals={str(checked(v)):v['sha256'] for v in manifest['evidence_files']}
    application=checked(manifest['application'])
    reader=checked(manifest['adapter_reader']);mapper=checked(manifest['adapter_mapper'])
    adapter_hash=hashlib.sha256(reader.read_bytes()+b'\0'+mapper.read_bytes()).hexdigest()
    acceptance_path=checked(manifest['acceptance'])
    acceptance=json.loads(acceptance_path.read_text())
    assert acceptance['adapter_sha256']==adapter_hash
    assert len(acceptance['builds'])==1 and acceptance['builds'][0]['application_sha256']==sha(application)
    accepted_version=acceptance['builds'][0]['version']
    image_source=checked(manifest['rgb']);sound_source=checked(manifest['pcm'])
    rgb=image_source.read_bytes();pcm=sound_source.read_bytes()
    width,height,count=manifest['width'],manifest['height'],manifest['source_frames']
    assert (width,height,count)==(128,72,100)
    frame_size=width*height*3
    assert len(rgb)==frame_size*count and len(pcm)==count*1920*4
    media=checked(manifest['media']);original_media=checked(manifest['original_media'])
    shutil.copy2(media,sources/'source.mkv');shutil.copy2(acceptance_path,sources/'acceptance.json')
    def identity(path):return {'path':path.relative_to(sources).as_posix(),'bytes':path.stat().st_size,'sha256':sha(path)}
    asset={'id':'source','path':'source.mkv','duration':time(4),'identity':{k:v for k,v in identity(sources/'source.mkv').items() if k!='path'}}
    bindings=[{'target_url':'native-media-0','asset':asset}]
    def call(command,error=None,**fields):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps({'command':command,**fields}).encode(),capture_output=True,timeout=120)
        reply=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and reply['error']['code']==error,(error,reply)
            rejected+=1;return reply['error']
        assert p.returncode==0 and reply['ok'],reply
        return reply['result']
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=90).stdout
    # Explicit media conversion must preserve the original source data, not only
    # produce an internally self-consistent timeline.
    for path in [media,original_media]:
        assert ff(['-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'])==rgb
        assert ff(['-i',str(path),'-vn','-f','s16le','-'])==pcm
    def expected(segments, data, unit):
        return b''.join(bytes(length*unit) if first is None else data[first*unit:(first+length)*unit] for first,length in segments)
    # Authored decisions are independent of the native capture and imported graph.
    full_rgb=expected([(7,25),(None,25),(51,25)],rgb,frame_size)
    full_pcm=expected([(7*1920,25*1920),(None,25*1920),(51*1920,25*1920)],pcm,4)
    muted_rgb=expected([(None,50),(51,25)],rgb,frame_size)
    muted_pcm=bytes(75*1920*4)
    def render(project,label,expected_rgb,expected_pcm):
        nonlocal frames,samples
        target=output/(label+'.mkv')
        result=call('render.run',project=project,input_root=str(sources),output_root=str(output),output=str(target))
        assert ff(['-i',str(target),'-an','-pix_fmt','rgb24','-f','rawvideo','-'])==expected_rgb,label
        assert ff(['-i',str(target),'-vn','-f','s16le','-'])==expected_pcm,label
        assert result['frames']==len(expected_rgb)//frame_size and result['samples']==len(expected_pcm)//4
        frames+=result['frames'];samples+=result['samples']
    requests=[];reports=[]
    assert [c['label'] for c in manifest['cases']]==['flat','disabled-muted','nested']
    for i,item in enumerate(manifest['cases']):
        native=checked(item['source']);transfer_path=checked(item['transfer']);checked(item['capture'])
        # Preserve the native suffix privately; no native format parsing occurs.
        native_copy=sources/('native-'+str(i)+native.suffix)
        transfer_copy=sources/('transfer-'+str(i)+'.json')
        shutil.copy2(native,native_copy);shutil.copy2(transfer_path,transfer_copy)
        transfer=json.loads(transfer_copy.read_text())
        assert transfer['producer']['adapter_sha256']==adapter_hash
        assert transfer['producer']['version']==accepted_version
        request={'source':identity(native_copy),'transfer':identity(transfer_copy),'acceptance':identity(sources/'acceptance.json'),
                 'input_root':str(sources),'media_root':str(sources),'id':'native-original','bindings':copy.deepcopy(bindings)}
        report=call('native.import',**request)
        assert not report['ready'] and report['project'] is None and not report['writes_files'] and not report['executes_adapter']
        assert {'appearance_and_color_omitted','audio_processing_omitted','metadata_and_links_omitted'}<={v['code'] for v in report['host_issues']}
        request['acknowledged_losses']=report['required_acknowledgements']
        ack=call('native.import',**request)
        assert ack['ready']==(i<2)
        if i==2:
            blockers=[v['id'] for v in ack['host_issues'] if v['blocking']]
            assert len(blockers)==2 and ack['project'] is None
            call('native.import',error='INVALID_LOSS_ACKNOWLEDGEMENT',**{**request,'acknowledged_losses':request['acknowledged_losses']+blockers})
        else:
            render(ack['project'],item['label'],full_rgb if i==0 else muted_rgb,full_pcm if i==0 else muted_pcm)
            # A partial acknowledgement must still produce no editable snapshot.
            partial=call('native.import',**{**request,'acknowledged_losses':request['acknowledged_losses'][:-1]})
            assert not partial['ready'] and partial['project'] is None
        requests.append(request);reports.append(ack)
    project=reports[0]['project'];request=requests[0]
    tracks=project['tracks']['tracks'];v=tracks[0];a=tracks[3]
    assert [(c['start'],c['source_in'],c['duration']) for c in v['clips']]==[(time(0),time(7,25),time(1)),(time(2),time(51,25),time(1))]
    assert [(c['start'],c['source_in'],c['duration']) for c in a['clips']]==[(time(0),time(7,25),time(1)),(time(2),time(51,25),time(1))]
    assert not reports[1]['project']['tracks']['tracks'][3]['enabled']
    assert any(v['code']=='unselected_sequences' for v in reports[1]['host_issues'])
    passed.append('native_projects.actual_native_captures_and_independent_editorial_output')
    passed.append('native_projects.explicit_omissions_disabled_mute_and_blocked_nesting')
    before={p.name:sha(p) for p in sources.iterdir()}
    # Reject exact-build and adapter changes; this is a one-build acceptance
    # matrix, not evidence that other application releases work.
    base=json.loads((sources/'transfer-0.json').read_text())
    def variant(label,mutate,code):
        value=copy.deepcopy(base);mutate(value);path=sources/(label+'.json');path.write_text(json.dumps(value))
        return call('native.import',error=code,**{**request,'transfer':identity(path)})
    variant('other-version',lambda d:d['producer'].update(version='unaccepted-build'),'UNSUPPORTED_NATIVE_BUILD')
    variant('other-application',lambda d:d['producer'].update(application_sha256='0'*64),'UNSUPPORTED_NATIVE_BUILD')
    variant('other-adapter',lambda d:d['producer'].update(adapter_sha256='0'*64),'NATIVE_ADAPTER_MISMATCH')
    variant('other-native-source',lambda d:d['source'].update(sha256='0'*64),'NATIVE_SOURCE_MISMATCH')
    variant('incomplete',lambda d:d.update(complete=False),'INVALID_NATIVE_TRANSFER')
    variant('missing-selection-loss',lambda d:d.update(sequence_count=2),'INVALID_NATIVE_TRANSFER')
    variant('duplicate-issue',lambda d:d['issues'].append(d['issues'][0]),'INVALID_NATIVE_TRANSFER')
    variant('foreign-issue',lambda d:d['issues'][0].update(id='arbitrary'),'INVALID_NATIVE_TRANSFER')
    variant('unknown-field',lambda d:d.update(unknown=True),'INVALID_JSON')
    call('native.import',error='INVALID_LOSS_ACKNOWLEDGEMENT',**{**request,'acknowledged_losses':request['acknowledged_losses']+['host:unknown']})
    call('native.import',error='INVALID_LOSS_ACKNOWLEDGEMENT',**{**request,'acknowledged_losses':request['acknowledged_losses']*2})
    for field in ['source','transfer','acceptance']:
        wrong={**request[field],'sha256':'0'*64}
        call('native.import',error='MEDIA_CHANGED',**{**request,field:wrong})
        wrong={**request[field],'path':'../'+request[field]['path']}
        call('native.import',error='INVALID_PATH',**{**request,field:wrong})
    call('native.import',error='LIMIT_EXCEEDED',**{**request,'transfer':{**request['transfer'],'bytes':4194305}})
    call('native.import',error='LIMIT_EXCEEDED',**{**request,'acceptance':{**request['acceptance'],'bytes':65537}})
    bad=copy.deepcopy(bindings);bad[0]['asset']['identity']['sha256']='0'*64
    call('native.import',error='MEDIA_CHANGED',**{**request,'bindings':bad})
    assert all(sha(sources/name)==digest for name,digest in before.items())
    assert all(sha(path)==digest for path,digest in originals.items())
    passed.append('native_projects.exact_build_adapter_source_and_boundary_guards')
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==MCP_TOOLS
        tool=next(t for t in catalog if t['name']=='cutbolt_native_import')
        Draft202012Validator(tool['inputSchema']).validate(request)
        assert tool['annotations']['readOnlyHint'] and tool['annotations']['idempotentHint']
        assert client.call('native.import',**request)['project']==project
        common={'store_root':str(store),'project_id':project['id']}
        client.call('session.create',store_root=str(store),project=project,request_id='create')
        operations=[move([v['clips'][1]['id']],25,backward=True)]
        preview=client.call('session.preview',**common,expected_revision=0,operations=operations)
        assert len(preview['clips'])==1 and preview['clips'][0]['clip_id']==v['clips'][1]['id']
        assert preview['clips'][0]['before']['timeline_start']==time(2)
        assert preview['clips'][0]['after']['timeline_start']==time(1)
        edit_request={**common,'expected_revision':0,'operations':operations,'request_id':'move'}
        receipt=client.call('session.apply',**edit_request);assert client.call('session.apply',**edit_request)==receipt
        saved=client.call('session.get',**common)
        render(saved,'saved-edit',expected([(7,25),(51,25),(None,25)],rgb,frame_size),full_pcm)
        client.call('session.undo',**common,expected_revision=saved['revision'],request_id='undo')
        restored=client.call('session.get',**common);render(restored,'restored',full_rgb,full_pcm)
    finally:client.close()
    assert all(sha(path)==digest for path,digest in originals.items())
    passed.append('native_projects.typed_proposals_saved_edits_replay_and_undo')
    report={'passed':passed,'native_cases':3,'accepted_exact_builds':1,'fresh_application_launch':False,
            'evidence_mode':'replayed_content_bound_public_interface_captures','frames_compared':frames,'samples_compared':samples,
            'rejections':rejected,'source_preserved':True,'adapter_sha256':adapter_hash,'application_sha256':sha(application),
            'fixture_manifest_sha256':sha(fixture),'appearance_equivalence_claimed':False}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);parser.add_argument('--fixture',type=Path,required=True)
    args=parser.parse_args();run(args.output,args.fixture)
