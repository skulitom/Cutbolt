"""Exercise the public indexed decoder against full explicit conform RGB, including sought GOPs."""
import hashlib
import json
import shutil
from pathlib import Path
import subprocess

from engine import BUILD
from footage_sources import identity
from source_clocks import decoded


def helper(root):
    source = root / 'direct-reader.rs'
    source.write_text(r'''
use cutbolt::{color::Transfer, direct::Video, media::{Control, Uncontrolled}, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{io::{self, Read}, path::Path, process::Command, sync::Mutex};
#[derive(Default)]
struct Commands(Mutex<Vec<Vec<String>>>);
impl Control for Commands {
    fn command(&self, program: &str, args: &[String]) -> Result<Command> {
        self.0.lock().unwrap().push(args.to_vec());
        Uncontrolled.command(program,args)
    }
}
fn main() {
    let mut input = String::new(); io::stdin().read_to_string(&mut input).unwrap();
    let requests: Vec<Value> = serde_json::from_str(&input).unwrap();
    let mut results = Vec::new();
    for request in requests {
        let control = Commands::default();
        let file = serde_json::from_value(request["file"].clone()).unwrap();
        let opened = Video::open(Path::new(request["root"].as_str().unwrap()),&file,&control);
        let video = match opened {
            Ok(video) => video,
            Err(error) => { results.push(json!({"error":error})); continue; }
        };
        let mutate = || {
            let path = Path::new(request["root"].as_str().unwrap()).join(&file.path);
            let mut bytes = std::fs::read(&path).unwrap();
            *bytes.last_mut().unwrap() ^= 1;
            std::fs::write(path,bytes).unwrap();
        };
        if request["mutation"] == "before" { mutate(); }
        let mut ranges = Vec::new();
        for range in request["ranges"].as_array().unwrap() {
            let first = range[0].as_u64().unwrap();
            let count = range[1].as_u64().unwrap();
            let mut hash = Sha256::new();
            let mut next = first;
            let result = video.read(first,count,Transfer::Bt709,&control,|frame,rgb| {
                assert_eq!(frame,next); next += 1; hash.update(rgb);
                if request["mutation"] == "during" && next == first + count { mutate(); }
                Ok(())
            });
            match result {
                Ok(receipt) => ranges.push(json!({"receipt":receipt,"sha256":format!("{:x}",hash.finalize())})),
                Err(error) => ranges.push(json!({"error":error})),
            }
        }
        results.push(json!({"frames":video.frames(),"ranges":ranges,"commands":control.0.into_inner().unwrap()}));
    }
    println!("{}",json!(results));
}
''', encoding='utf-8')
    binary = root / 'direct-reader.exe'
    args = ['rustc','--edition=2024',str(source),'-o',str(binary),'-L',str(BUILD/'deps'),
            '--extern','cutbolt='+str(BUILD/'libcutbolt.rlib')]
    for name in ('serde_json','sha2'):
        library = max((BUILD/'deps').glob('lib'+name+'-*.rlib'),key=lambda p:p.stat().st_mtime)
        args += ['--extern',name+'='+str(library)]
    subprocess.run(args,check=True,capture_output=True,timeout=90)
    return binary


def verify(root, manifest, indexed):
    requests, expected = [], []
    for name, case in manifest['cases'].items():
        if name == 'h264-20min':
            continue
        item = indexed[name]
        file = {'path':case['path'],**identity(root/'sources'/case['path'])}
        request = {'root':str(root/'sources'),'file':file,'ranges':[]}
        if item['index']['eligible']:
            n = item['frames']
            key = case['gop']
            windows = sorted({(0,1),(1,1),(key,1),(key+1,1),(n-1,1),(n-2,2),
                              (key-3,8),(n-15,10),(0,n)})
            request['ranges'] = [*windows,(n,1),(0,0),(2**64-1,2)]
            reference = decoded(root/'conform'/(name+'.mkv'),True)
            size = case['size'][0]*case['size'][1]*3
            expected.append([hashlib.sha256(reference[a*size:(a+b)*size]).hexdigest() for a,b in windows])
        else:
            expected.append(None)
        requests.append(request)
    binary = helper(root)
    process = subprocess.run([str(binary)],input=json.dumps(requests),capture_output=True,
                             text=True,encoding='utf-8',timeout=1800)
    assert process.returncode == 0, process.stderr
    output = json.loads(process.stdout)
    (root/'direct-library-observed.json').write_text(json.dumps(output,indent=2),encoding='utf-8')
    reports = {}
    for request, hashes, result in zip(requests,expected,output,strict=True):
        name = Path(request['file']['path']).stem
        if hashes is None:
            assert result['error']['code'] == 'UNSUPPORTED_MEDIA', (name,result)
            reports[name] = {'rejection':result['error']['code']}
            continue
        assert len(result['ranges']) == len(hashes)+3
        for (first,count), expected_hash, actual in zip(request['ranges'],hashes,result['ranges']):
            assert actual.get('sha256') == expected_hash, (name,first,count,actual,expected_hash)
            receipt = actual['receipt']
            assert (receipt['first'],receipt['frames']) == (first,count), receipt
            if first >= 2*manifest['cases'][name]['gop']:
                assert receipt['seek_frame'] > 0 and receipt['seek_seconds'] is not None, receipt
        assert all(r['error']['code']=='INVALID_RANGE' for r in result['ranges'][-3:]), result
        commands = [c for c in result['commands'] if 'rawvideo' in c]
        assert len(commands) == len(hashes), (name,len(commands),len(hashes))
        assert all('gbrp' not in c and c[c.index('-pix_fmt')+1] in ('yuv420p','yuvj420p') for c in commands)
        assert any('-ss' in c for c in commands) == (result['frames'] > 2*manifest['cases'][name]['gop']), name
        assert identity(Path(request['root'])/request['file']['path']) == {
            k:request['file'][k] for k in ('sha256','bytes')}
        reports[name] = {'windows':result['ranges'][:-3],'native_decoder_calls':len(commands),'sources_preserved':True}
    original = root/'sources'/manifest['cases']['h264-25']['path']
    faults = root/'changed-sources'
    faults.mkdir(exist_ok=True)
    checks = []
    for mode in ('before','during'):
        copy = faults/(mode+'.mp4')
        shutil.copy2(original,copy)
        checks.append({'root':str(faults),'file':{'path':copy.name,**identity(copy)},
                       'ranges':[[0,manifest['cases']['h264-25']['encoded_frames']]],'mutation':mode})
    mutations = subprocess.run([str(binary)],input=json.dumps(checks),capture_output=True,
                              text=True,encoding='utf-8',timeout=180)
    assert mutations.returncode == 0, mutations.stderr
    for mode, result in zip(('before','during'),json.loads(mutations.stdout),strict=True):
        assert result['ranges'][0]['error']['code'] == 'MEDIA_CHANGED', (mode,result)
        assert len([c for c in result['commands'] if 'rawvideo' in c]) == (1 if mode=='during' else 0)
    reports['source_changes'] = {'before_decode':'MEDIA_CHANGED','during_decode':'MEDIA_CHANGED'}
    return reports
