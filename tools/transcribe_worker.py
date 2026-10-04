"""Original optional offline speech adapter; all recognizer/model code stays external.

The fixed profile returns estimated acoustic evidence and explicit contextual cut
intervals. Nothing here authenticates speech or substitutes for reviewing a cut.
Run only through transcribe_supervisor.py in the selected Linux namespaces.
"""
import gc
import hashlib
import importlib.metadata as metadata
import json
import math
import os
from pathlib import Path
import resource
import socket
import sys
import time
import unicodedata
import wave

PROTOCOL = 'cutbolt-transcription-v1'
PROFILE = 'local-en-el-context-v1'
MODEL = (483617219, '9ecf779972d90ba49c06d968637d720dd632c55bbf19d441fb42bf17a411e794')
ALIGNMENT = {
    'en': {
        'config.json': (1596, 'd3ec255c063d9f95057b553b19c20135b259875834a4fe9deb218a6be25b4cf3'),
        'model.safetensors': (377607901, '8aa76ab2243c81747a1f832954586bc566090c83a0ac167df6f31f0fa917d74a'),
        'preprocessor_config.json': (159, 'b225d617c025463b9e157e06afea8b90dc7078fc70b013c533328423e0486b4a'),
        'vocab.json': (291, '19727f8944fe6459fc3f240ae2c198395b740f6a029bd23e06656266b83bcf64'),
    },
    'el': {
        'config.json': (1811, 'e7176181819df26877beeeba8b1abe9dd7ff5cea4b0562ae8031bd09e8939e12'),
        'pytorch_model.bin': (1262101912, '65f2b38c59498e261b55da3d81f1156c22531b798055174e62a4343bbc22951f'),
        'preprocessor_config.json': (214, '60ca5a31e13f69ee2fbf147504c8676db5f6398fd7a6b12294341dff838edfcf'),
        'vocab.json': (405, '6990910750304a4ce9af15462f2cbb3494ace69311f631eb5993d5bb3df89a79'),
    },
}
VERSIONS = {'openai-whisper':'20250625','torch':'2.3.1+cu121','torchaudio':'2.3.1+cu121',
    'numpy':'1.26.4','numba':'0.60.0','llvmlite':'0.43.0','tiktoken':'0.7.0',
    'more-itertools':'8.10.0','tqdm':'4.66.4','transformers':'4.44.2',
    'tokenizers':'0.19.1','huggingface-hub':'0.24.7','safetensors':'0.4.5'}


class Failure(Exception):
    def __init__(self, code, message):
        self.code, self.message = code, message


def require(condition, code, message):
    if not condition:
        raise Failure(code, message)


def fields(value, names):
    require(type(value) is dict and set(value) == set(names), 'INVALID_REQUEST', 'Missing or unknown worker fields')


def digest(path):
    value = hashlib.sha256()
    with path.open('rb') as stream:
        while chunk := stream.read(1048576):
            value.update(chunk)
    return value.hexdigest()


def checked_file(name, identity):
    require(type(name) is str and Path(name).is_absolute(), 'INVALID_PATH', 'Expected an absolute local path')
    path = Path(name)
    require(path.is_file(), 'MODEL_UNAVAILABLE', 'Required local file is missing')
    require(path.stat().st_size == identity[0] and digest(path) == identity[1], 'IDENTITY_MISMATCH', 'Required local file identity differs')
    return path.resolve(strict=True)


def milli(value):
    number = float(value)
    require(math.isfinite(number) and 0 <= number <= 1, 'INVALID_ALIGNMENT', 'Invalid model score')
    return int(math.floor(number * 1000 + .5))


def acoustic_edges(pcm, intervals, np):
    """Original RMS islands anchored by acoustic labels, bounded by neighbours."""
    padded = np.pad(pcm.astype(np.float64), (0, (-len(pcm)) % 80))
    rms = np.sqrt(np.mean(padded.reshape(-1, 80) ** 2, axis=1))
    output = []
    for i, (start, end) in enumerate(intervals):
        a, b = start // 80, min(len(rms), (end + 79) // 80)
        low = max(0, start - 6400, intervals[i-1][1] if i else 0) // 80
        high = min(len(pcm), end + 6400, intervals[i+1][0] if i+1 < len(intervals) else len(pcm)) // 80
        active = rms[low:high] >= max(.002, float(rms[a:b].max()) * .03)
        on = np.flatnonzero(active)
        for left, right in zip(on, on[1:]):
            if right-left-1 <= 8:
                active[left:right+1] = True
        spans = []
        for index in np.flatnonzero(active):
            if spans and spans[-1][1] == index:
                spans[-1][1] = int(index+1)
            else:
                spans.append([int(index), int(index+1)])
        touched = [(lo+low, hi+low) for lo,hi in spans if hi+low > a and lo+low < b]
        output.append([min(start,touched[0][0]*80), min(len(pcm),max(end,touched[-1][1]*80))] if touched else [start,end])
    partition(output)
    return output


def partition(intervals):
    for left, right in zip(intervals, intervals[1:]):
        if left[1] > right[0]:
            boundary = (left[1] + right[0]) // 2
            left[1], right[0] = boundary, boundary


def contextual(acoustic, count):
    output = [[max(acoustic[i-1][1] if i else 0,start-1280),
               min(acoustic[i+1][0] if i+1<len(acoustic) else count,end+320)]
              for i,(start,end) in enumerate(acoustic)]
    partition(output)
    require(all(0 <= a < b <= count for a,b in output), 'INVALID_ALIGNMENT', 'Invalid contextual word intervals')
    return output


def recognition_blocks(pcm, np):
    """Disjoint source windows; prefer a real quiet gap before model context ends.

    Full <=30s inputs retain their original inference path. Longer inputs split
    at the midpoint of the longest >=200ms RMS-quiet gap between 8s and 14s;
    ties prefer the later gap. With no qualifying gap, use an explicit 12s cut.
    This reduces a recognizer's internal timestamp-based seek omissions while
    making every source interval and hard-cut limitation inspectable.
    """
    count=len(pcm);first=0;blocks=[]
    if count<=480000:return [(0,count,'source_end')]
    padded=np.pad(pcm.astype(np.float64),(0,(-count)%80))
    rms=np.sqrt(np.mean(padded.reshape(-1,80)**2,axis=1))
    while count-first>240000:
        low=(first+128000+79)//80;high=(first+224000)//80
        spans=[];begin=None
        for index in range(low,high+1):
            quiet=index<high and rms[index]<.002
            if quiet and begin is None:begin=index
            if not quiet and begin is not None:
                if index-begin>=40:spans.append((begin,index))
                begin=None
        if spans:
            a,b=max(spans,key=lambda interval:(interval[1]-interval[0],interval[1]))
            end=(a+b)*40;policy='quiet_gap'
        else:end=first+192000;policy='hard_12s'
        blocks.append((first,end,policy));first=end
    blocks.append((first,count,'source_end'))
    return blocks


def run(request):
    started = time.monotonic()
    fields(request, ['protocol','source_path','source_sha256','source_bytes','sample_count','model_path','alignment_root','language','threads'])
    require(request['protocol'] == PROTOCOL, 'INVALID_PROTOCOL', 'Unsupported speech worker protocol')
    require(request['language'] in ALIGNMENT, 'UNSUPPORTED_LANGUAGE', 'This profile supports en and el')
    require(type(request['threads']) is int and 1 <= request['threads'] <= 8, 'INVALID_REQUEST', 'Threads must be 1..8')
    require(type(request['sample_count']) is int and 400 <= request['sample_count'] <= 1920000, 'INPUT_LIMIT', 'Analysis requires 25 ms to 120 seconds')
    require(type(request['source_bytes']) is int and 44 <= request['source_bytes'] <= 3840044, 'INPUT_LIMIT', 'Invalid analysis WAV size')
    interfaces = [name for _,name in socket.if_nameindex()]
    require(interfaces == ['lo'] and os.readlink('/proc/1/ns/pid') == os.readlink('/proc/self/ns/pid'), 'ISOLATION_REQUIRED', 'Local speech requires its isolated network/PID namespace')
    network_attempts = []
    def deny(*args, **kwargs):
        network_attempts.append(True)
        raise Failure('NETWORK_DISABLED', 'Networking is disabled for local transcription')
    socket.socket.connect = deny
    socket.socket.connect_ex = deny
    socket.create_connection = deny
    socket.getaddrinfo = deny
    for key in ['OMP_NUM_THREADS','MKL_NUM_THREADS','OPENBLAS_NUM_THREADS','NUMBA_NUM_THREADS']:
        os.environ[key] = str(request['threads'])
    os.environ.update(HF_HUB_OFFLINE='1',TRANSFORMERS_OFFLINE='1',HF_HUB_DISABLE_TELEMETRY='1')
    versions = {name:metadata.version(name) for name in VERSIONS}
    require(versions == VERSIONS, 'RUNTIME_VERSION', 'Installed external speech runtime does not match the selected profile')
    source = checked_file(request['source_path'], (request['source_bytes'],request['source_sha256']))
    require(source.parent == Path(__file__).resolve().parent and source.name == 'analysis.wav', 'INVALID_PATH', 'Worker reads only its owned analysis file')
    model_path = checked_file(request['model_path'], MODEL)
    root = Path(request['alignment_root'])
    require(root.is_absolute() and root.is_dir(), 'MODEL_UNAVAILABLE', 'Explicit local alignment root required')
    files = {name:checked_file(str(root/name),value) for name,value in ALIGNMENT[request['language']].items()}
    with wave.open(str(source),'rb') as stream:
        require((stream.getnchannels(),stream.getsampwidth(),stream.getframerate(),stream.getcomptype(),stream.getnframes()) == (1,2,16000,'NONE',request['sample_count']), 'UNSUPPORTED_AUDIO', 'Expected the bound mono PCM16 analysis file at 16 kHz')
        pcm_bytes = stream.readframes(stream.getnframes())
    require(len(pcm_bytes) == request['sample_count']*2, 'UNSUPPORTED_AUDIO', 'Truncated analysis data')
    require(any(pcm_bytes), 'NO_WORDS', 'The selected analysis interval is digital silence')
    import numpy as np
    import torch
    import torchaudio
    import whisper
    from transformers import Wav2Vec2Config, Wav2Vec2ForCTC, Wav2Vec2FeatureExtractor
    from safetensors.torch import load_file
    torch.set_num_threads(request['threads']); torch.set_num_interop_threads(1)
    require(torch.cuda.is_available(), 'DEVICE_UNAVAILABLE', 'This selected profile requires a local CUDA device')
    torch.cuda.reset_peak_memory_stats()
    pcm = np.frombuffer(pcm_bytes,dtype='<i2').astype(np.float32)/32768
    model = whisper.load_model(str(model_path),device='cuda')
    words=[];recognition_windows=[]
    for first,end,policy in recognition_blocks(pcm,np):
        word_first=len(words)
        if np.any(pcm[first:end]):
            result = model.transcribe(pcm[first:end],language=request['language'],task='transcribe',word_timestamps=True,
                fp16=True,temperature=0.,beam_size=5,condition_on_previous_text=False,verbose=None)
            words.extend(word for segment in result['segments'] for word in segment['words'])
            del result
        recognition_windows.append({'start_sample':first,'end_sample':end,'end_policy':policy,
            'first_word':word_first,'end_word':len(words)})
    require(0 < len(words) <= 2048, 'NO_WORDS' if not words else 'RESULT_LIMIT', 'Recognition requires 1..2048 words')
    del model
    gc.collect(); torch.cuda.empty_cache()
    recognized = time.monotonic()
    config = Wav2Vec2Config.from_dict(json.loads(files['config.json'].read_text(encoding='utf-8')))
    aligner = Wav2Vec2ForCTC(config)
    weight_name = 'model.safetensors' if request['language'] == 'en' else 'pytorch_model.bin'
    state = load_file(str(files[weight_name])) if request['language'] == 'en' else torch.load(files[weight_name],map_location='cpu',weights_only=True)
    prefix = 'wav2vec2.encoder.pos_conv_embed.conv.'
    for old,new in [('weight_g','parametrizations.weight.original0'),('weight_v','parametrizations.weight.original1')]:
        require(prefix+new not in state and prefix+old in state, 'MODEL_FORMAT', 'Unexpected alignment weight normalization format')
        state[prefix+new] = state.pop(prefix+old)
    incompatible = aligner.load_state_dict(state,strict=False)
    require(not incompatible.unexpected_keys and incompatible.missing_keys in ([],['wav2vec2.masked_spec_embed']), 'MODEL_FORMAT', 'Alignment weights do not match the public model definition')
    del state
    aligner = aligner.eval().to('cuda')
    extractor = Wav2Vec2FeatureExtractor(**json.loads(files['preprocessor_config.json'].read_text(encoding='utf-8')))
    vocab = json.loads(files['vocab.json'].read_text(encoding='utf-8'))
    labels, ranges = [], []
    for word in words:
        text = word['word']
        require(type(text) is str and len(text.encode('utf-8')) <= 512 and len(text.split()) == 1 and not any(unicodedata.category(c) == 'Cc' for c in text), 'INVALID_ALIGNMENT', 'Invalid recognized word text')
        require(not any(c.isnumeric() for c in text), 'UNSUPPORTED_ALIGNMENT_TEXT', 'Numeric word spelling is not supported by this acoustic profile')
        clean = ''.join(c for c in unicodedata.normalize('NFC',text.upper()) if c.isalpha() or c == "'")
        require(clean and all(c in vocab for c in clean), 'UNSUPPORTED_ALIGNMENT_TEXT', 'Recognized text contains unsupported acoustic labels; use an explicit correction workflow')
        if labels: labels.append(vocab['|'])
        first = len(labels); labels.extend(vocab[c] for c in clean); ranges.append((first,len(labels)))
    require(len(labels) <= 16384, 'RESULT_LIMIT', 'Acoustic target exceeds 16384 labels')
    stride, receptive = 1, 1
    for kernel, step in zip(config.conv_kernel,config.conv_stride):
        receptive += (kernel-1)*stride; stride *= step
    require((stride,receptive) == (320,400), 'MODEL_FORMAT', 'Unexpected acoustic convolution clock')
    count = len(pcm); frame_count = (count-receptive)//stride+1
    emissions, blocks = [], []
    # Preserve whole-fixture inference <=30s. Longer inputs have 24s kept tiles
    # with 1s context on either side, aligned to the exact convolution clock.
    tile = frame_count if count <= 480000 else 1200
    for first in range(0,frame_count,tile):
        end = min(first+tile,frame_count)
        low = max(0,first*stride-16000)
        high = min(count,(end-1)*stride+receptive+16000)
        inputs = extractor(pcm[low:high],sampling_rate=16000,return_tensors='pt').to('cuda')
        with torch.inference_mode():
            logits = aligner(**inputs).logits.log_softmax(-1).cpu()
        offset = first-low//stride
        kept = logits[:,offset:offset+end-first].clone()
        require(kept.shape[1] == end-first, 'INVALID_ALIGNMENT', 'Acoustic chunk clock mismatch')
        emissions.append(kept); blocks.append({'start_sample':low,'end_sample':high,'first_frame':first,'end_frame':end})
        del logits, inputs
    emission = torch.cat(emissions,dim=1)
    repeated = sum(a == b for a,b in zip(labels,labels[1:]))
    require(len(labels)+repeated <= frame_count, 'INVALID_ALIGNMENT', 'Too many acoustic labels for this source interval')
    raw, confidences, alignment_windows = [], [], []
    # Keep acoustic words in the disjoint source window that actually produced
    # them. Repeated text must not allow a global CTC path to shift a recognized
    # phrase into another window after the recognizer omits a word or phrase.
    for window in recognition_windows:
        word_first,word_end=window['first_word'],window['end_word']
        if word_first==word_end:continue
        label_first,label_end=ranges[word_first][0],ranges[word_end-1][1]
        target=labels[label_first:label_end]
        first=max(0,(window['start_sample']-receptive//2+stride-1)//stride)
        end=min(frame_count,(window['end_sample']-receptive//2+stride-1)//stride)
        repeated=sum(a==b for a,b in zip(target,target[1:]))
        require(len(target)+repeated<=end-first,'INVALID_ALIGNMENT','Recognized window has too many labels for its acoustic clock')
        alignment,scores = torchaudio.functional.forced_align(emission[:,first:end],torch.tensor([target]),blank=config.pad_token_id)
        spans = torchaudio.functional.merge_tokens(alignment[0],scores[0].exp(),blank=config.pad_token_id)
        require([s.token for s in spans] == target, 'INVALID_ALIGNMENT', 'Acoustic path did not cover every label')
        alignment_windows.append({'first_frame':first,'end_frame':end,'first_word':word_first,'end_word':word_end})
        for i in range(word_first,word_end):
            a,b=ranges[i];selected=spans[a-label_first:b-label_first]
            raw.append([max(window['start_sample'],(first+selected[0].start)*stride),
                min(window['end_sample'],(first+selected[-1].end-1)*stride+receptive)])
            confidences.append(milli(sum(s.score*(s.end-s.start) for s in selected)/sum(s.end-s.start for s in selected)))
    acoustic = acoustic_edges(pcm,raw,np)
    context = contextual(acoustic,count)
    output = [{'id':f'w{i:04}','text':word['word'],'start_sample':a,'end_sample':b,
        'probability_milli':milli(word['probability']),'ctc_start_sample':raw[i][0],'ctc_end_sample':raw[i][1],
        'acoustic_start_sample':acoustic[i][0],'acoustic_end_sample':acoustic[i][1],'alignment_score_milli':confidences[i]}
        for i,(word,(a,b)) in enumerate(zip(words,context))]
    checked_file(str(source),(request['source_bytes'],request['source_sha256']))
    checked_file(str(model_path),MODEL)
    for name,value in ALIGNMENT[request['language']].items(): checked_file(str(files[name]),value)
    peak_cpu = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss*1024
    peak_gpu = torch.cuda.max_memory_allocated()
    require(peak_cpu <= 6*1024**3 and peak_gpu <= 8*1024**3, 'RESOURCE_LIMIT', 'Speech profile exceeded its memory gate')
    return {'protocol':PROTOCOL,'profile':PROFILE,'source_sha256':request['source_sha256'],'sample_count':count,
        'language':request['language'],'words':output,'versions':versions,'alignment_files':ALIGNMENT[request['language']],
        'model_sha256':MODEL[1],'network_interfaces':interfaces,'python_network_attempts':len(network_attempts),
        'peak_cpu_bytes':peak_cpu,'peak_cuda_bytes':peak_gpu,'recognition_seconds':recognized-started,
        'total_seconds':time.monotonic()-started,'alignment_blocks':blocks,
        'recognition_blocks':recognition_windows,
        'alignment_windows':alignment_windows,
        'context_samples':{'leading':1280,'trailing':320},'review_required':True}


if __name__ == '__main__':
    try:
        data = sys.stdin.buffer.read(65537)
        require(len(data) <= 65536,'REQUEST_LIMIT','Worker request exceeds 64 KiB')
        value = run(json.loads(data))
        encoded = json.dumps({'ok':True,'result':value},ensure_ascii=False,allow_nan=False).encode('utf-8')
        require(len(encoded) <= 1048576,'RESULT_LIMIT','Worker response exceeds one MiB')
        sys.stdout.buffer.write(encoded+b'\n')
    except Failure as exc:
        print(json.dumps({'ok':False,'error':{'code':exc.code,'message':exc.message}}))
        raise SystemExit(1)
    except metadata.PackageNotFoundError:
        print(json.dumps({'ok':False,'error':{'code':'RUNTIME_UNAVAILABLE','message':'Required external speech package is missing'}}))
        raise SystemExit(1)
    except Exception as exc:
        print(json.dumps({'ok':False,'error':{'code':'WORKER_FAILED','message':str(exc)[:512]}}))
        raise SystemExit(1)
