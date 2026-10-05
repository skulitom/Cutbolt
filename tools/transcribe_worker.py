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

PROTOCOL = 'cutbolt-transcription-v2'
# One launch analyses up to this many owned analysis files, each with its own outcome.
MAX_ITEMS = 16
RESULT_BYTES = 16 * 1048576
ANALYSIS_NAMES = frozenset(['analysis.wav'] + [f'analysis-{n}.wav' for n in range(MAX_ITEMS)])
# Failures of the shared setup or models end the whole launch; others belong to one item.
FATAL = frozenset(['MODEL_FORMAT','MODEL_UNAVAILABLE','DEVICE_UNAVAILABLE','RESOURCE_LIMIT','NETWORK_DISABLED',
    'ISOLATION_REQUIRED','RUNTIME_VERSION'])
PROFILE = 'local-en-el-context-v1'
ALIGN_PROFILE = 'local-en-el-align-v1'
WEIGHTS = {'en':'model.safetensors','el':'pytorch_model.bin'}
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


def energy(pcm, np):
    """RMS of each 5 ms analysis frame."""
    padded=np.pad(pcm.astype(np.float64),(0,(-len(pcm))%80))
    return np.sqrt(np.mean(padded.reshape(-1,80)**2,axis=1))


def quiet_end(rms, first):
    """End of the window from `first`: the middle of the longest >=200ms RMS-quiet
    gap 8-14s on (ties prefer the later gap), else an explicit 12s cut."""
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
        return (a+b)*40,'quiet_gap'
    return first+192000,'hard_12s'


def recognition_blocks(pcm, np):
    """Disjoint source windows; prefer a real quiet gap before model context ends.

    Full <=30s inputs retain their original inference path. Longer inputs split
    at the midpoint of the longest >=200ms RMS-quiet gap between 8s and 14s;
    ties prefer the later gap. With no qualifying gap, use an explicit 12s cut,
    which run() first tries to move to a gap between recognized words (word_gap).
    This reduces a recognizer's internal timestamp-based seek omissions while
    making every source interval and hard-cut limitation inspectable.
    """
    count=len(pcm);first=0;blocks=[]
    if count<=480000:return [(0,count,'source_end')]
    rms=energy(pcm,np)
    while count-first>240000:
        end,policy=quiet_end(rms,first)
        blocks.append((first,end,policy));first=end
    blocks.append((first,count,'source_end'))
    return blocks


def sample(seconds, first, end):
    """A recognizer time in seconds from `first`, on the analysis clock within [first, end]."""
    return min(end,max(first,first+int(math.floor(float(seconds)*16000+.5))))


def word_gap(raw, first):
    """Cut for a window with no quiet gap, from a 14s recognition pass at `first`.

    Music beds and room tone leave no RMS-quiet gap, and a fixed 12s cut then
    splits spoken words. Recognized token boundaries are still visible: take the
    widest gap between consecutive tokens (the last token's gap runs to the pass
    end) that meets 8-14s, and cut at the middle of its part inside that span;
    ties prefer the later gap. None when no token boundary lies inside.
    """
    low,high=first+128000,first+224000
    times=[(sample(w['start'],first,high),sample(w['end'],first,high)) for w in raw]
    gaps=[(a[1],b[0]) for a,b in zip(times,times[1:])]+[(times[-1][1] if times else first,high)]
    best=None
    for a,b in gaps:
        a,b=max(a,low),min(max(a,b),high)
        if a<=b and a<high and (best is None or (b-a,(a+b)//2)>=best):best=(b-a,(a+b)//2)
    return None if best is None else best[1]


NOTES=frozenset('♩♪♫♬♭♮♯\U0001f3b5\U0001f3b6\U0001f3bc')
BRACKETS={'[':']','(':')','{':'}'}


def speech(raw, first, end):
    """Separate recognized speech from text that is not speech.

    Recognizers annotate sound in brackets ("[Music]", "(upbeat music)", over up to
    eight tokens) or with music symbols; these are dropped and reported. A token with
    no letter or digit (the "-" of a word cut off at a window edge) joins the speech
    token it is written against, or is reported when there is none. Word text loses
    the recognizer's surrounding whitespace.
    """
    kept,notes,i=[],[],0
    def note(tokens,kind):
        start=sample(tokens[0]['start'],first,end)
        notes.append({'text':' '.join(w['word'].strip() for w in tokens),'kind':kind,
            'start_sample':start,'end_sample':max(start,sample(tokens[-1]['end'],first,end))})
    while i<len(raw):
        word=raw[i];text=word['word'].strip()
        close=BRACKETS.get(text[:1])
        ends=[j for j in range(i,min(i+8,len(raw))) if raw[j]['word'].strip().rstrip('.,!?;:…').endswith(close)] if close else []
        if ends:
            note(raw[i:ends[0]+1],'annotation');i=ends[0]+1;continue
        joined=kept and kept[-1]['index']==i-1 and not word['word'][:1].isspace()
        if any(c.isalnum() for c in text):kept.append({**word,'word':text,'index':i})
        elif any(c in NOTES for c in text):note([word],'music')
        elif text and joined:
            last=kept[-1];kept[-1]={**last,'word':last['word']+text,'index':i,
                'probability':min(last['probability'],word['probability'])}
        elif text:note([word],'symbols')
        i+=1
    return kept,notes


def letters(text, vocab):
    """Acoustic labels of a word: its letters and apostrophes in upper case. A letter
    the vocabulary lacks falls back to its base letter (É to E) when that one exists."""
    output = []
    for c in unicodedata.normalize('NFC',text.upper()):
        if not (c.isalpha() or c == "'"): continue
        base = unicodedata.normalize('NFD',c)[0]
        output.append(c if c in vocab or base not in vocab else base)
    return ''.join(output)


def load_aligner(files, language):
    """The acoustic alignment model, its feature extractor, vocabulary and configuration."""
    import torch
    from transformers import Wav2Vec2Config, Wav2Vec2ForCTC, Wav2Vec2FeatureExtractor
    from safetensors.torch import load_file
    config = Wav2Vec2Config.from_dict(json.loads(files['config.json'].read_text(encoding='utf-8')))
    aligner = Wav2Vec2ForCTC(config)
    weight_name = WEIGHTS[language]
    state = load_file(str(files[weight_name])) if language == 'en' else torch.load(files[weight_name],map_location='cpu',weights_only=True)
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
    return aligner, extractor, vocab, config


def align(pcm, words, windows, loaded, given):
    """Acoustic word intervals: CTC alignment of each window's words inside that window."""
    import numpy as np
    import torch
    import torchaudio
    aligner, extractor, vocab, config = loaded
    labels, ranges = [], []
    origin = 'Text' if given else 'Recognized'
    for word in words:
        text = word['word']
        require(type(text) is str and len(text.encode('utf-8')) <= 512 and len(text.split()) == 1 and not any(unicodedata.category(c) == 'Cc' for c in text), 'INVALID_ALIGNMENT', 'Invalid recognized word text')
        require(not any(c.isnumeric() for c in text), 'UNSUPPORTED_ALIGNMENT_TEXT', f'{origin} word {text[:64]!r} uses digits; this acoustic profile aligns numbers spelled out')
        clean = letters(text,vocab)
        require(clean and all(c in vocab for c in clean), 'UNSUPPORTED_ALIGNMENT_TEXT', f'{origin} word {text[:64]!r} has letters this acoustic profile cannot align; correct the text')
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
    for window in windows:
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
        'probability_milli':None if word['probability'] is None else milli(word['probability']),'ctc_start_sample':raw[i][0],'ctc_end_sample':raw[i][1],
        'acoustic_start_sample':acoustic[i][0],'acoustic_end_sample':acoustic[i][1],'alignment_score_milli':confidences[i]}
        for i,(word,(a,b)) in enumerate(zip(words,context))]
    return output, blocks, alignment_windows


def checked_item(item):
    fields(item, ['source_path','source_sha256','source_bytes','sample_count','text'])
    given = item['text']
    require(given is None or (type(given) is list and 0 < len(given) <= 2048 and all(type(t) is str and len(t.encode('utf-8')) <= 512
        and len(t.split()) == 1 and t == t.strip() for t in given)), 'INVALID_REQUEST', 'Text to align must be 1..2048 single words')
    require(type(item['sample_count']) is int and 400 <= item['sample_count'] <= 1920000, 'INPUT_LIMIT', 'Analysis requires 25 ms to 120 seconds')
    require(type(item['source_bytes']) is int and 44 <= item['source_bytes'] <= 3840044, 'INPUT_LIMIT', 'Invalid analysis WAV size')


def run(request):
    started = time.monotonic()
    fields(request, ['protocol','items','model_path','alignment_root','language','threads'])
    require(request['protocol'] == PROTOCOL, 'INVALID_PROTOCOL', 'Unsupported speech worker protocol')
    items = request['items']
    require(type(items) is list and 1 <= len(items) <= MAX_ITEMS, 'INVALID_REQUEST', 'A launch analyses 1..16 items')
    for item in items: checked_item(item)
    recognizing = any(item['text'] is None for item in items)
    require(recognizing == (request['model_path'] is not None), 'INVALID_REQUEST', 'Recognition takes a model; text alignment alone does not')
    require(request['language'] in ALIGNMENT, 'UNSUPPORTED_LANGUAGE', 'This profile supports en and el')
    require(type(request['threads']) is int and 1 <= request['threads'] <= 8, 'INVALID_REQUEST', 'Threads must be 1..8')
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
    model_path = checked_file(request['model_path'], MODEL) if recognizing else None
    root = Path(request['alignment_root'])
    require(root.is_absolute() and root.is_dir(), 'MODEL_UNAVAILABLE', 'Explicit local alignment root required')
    files = {name:checked_file(str(root/name),value) for name,value in ALIGNMENT[request['language']].items()}
    import numpy as np
    import torch
    torch.set_num_threads(request['threads']); torch.set_num_interop_threads(1)
    require(torch.cuda.is_available(), 'DEVICE_UNAVAILABLE', 'This selected profile requires a local CUDA device')
    torch.cuda.reset_peak_memory_stats()
    # Each model is loaded at most once per launch and shared by every item.
    loaded = {}
    def recognizer():
        if 'whisper' not in loaded:
            import whisper
            loaded['whisper'] = whisper.load_model(str(model_path),device='cuda')
        return loaded['whisper']
    def aligner():
        if 'aligner' not in loaded:
            loaded['aligner'] = load_aligner(files,request['language'])
        return loaded['aligner']

    def analyse(item):
        item_started = time.monotonic()
        given = item['text']
        source = checked_file(item['source_path'], (item['source_bytes'],item['source_sha256']))
        require(source.parent == Path(__file__).resolve().parent and source.name in ANALYSIS_NAMES, 'INVALID_PATH', 'Worker reads only its owned analysis files')
        with wave.open(str(source),'rb') as stream:
            require((stream.getnchannels(),stream.getsampwidth(),stream.getframerate(),stream.getcomptype(),stream.getnframes()) == (1,2,16000,'NONE',item['sample_count']), 'UNSUPPORTED_AUDIO', 'Expected the bound mono PCM16 analysis file at 16 kHz')
            pcm_bytes = stream.readframes(stream.getnframes())
        require(len(pcm_bytes) == item['sample_count']*2, 'UNSUPPORTED_AUDIO', 'Truncated analysis data')
        require(any(pcm_bytes), 'NO_WORDS', 'The selected analysis interval is digital silence')
        pcm = np.frombuffer(pcm_bytes,dtype='<i2').astype(np.float32)/32768
        count = len(pcm)
        words=[];notes=[];recognition_windows=[]
        if given is not None:
            # Known text (a narration script): no recognition, one window over the whole analysis.
            words=[{'word':text,'probability':None} for text in given]
            recognition_windows.append({'start_sample':0,'end_sample':count,'end_policy':'given_text','first_word':0,'end_word':len(words)})
        else:
            model = recognizer()
            def recognize(first,end):
                if not np.any(pcm[first:end]):return []
                result = model.transcribe(pcm[first:end],language=request['language'],task='transcribe',word_timestamps=True,
                    fp16=True,temperature=0.,beam_size=5,condition_on_previous_text=False,verbose=None)
                return [word for segment in result['segments'] for word in segment['words']]
            rms = energy(pcm,np) if count>480000 else None
            first = 0
            while first<count:
                end,policy = (count,'source_end') if count<=480000 or count-first<=240000 else quiet_end(rms,first)
                raw = None
                if policy=='hard_12s':
                    # Recognize 14s, then cut between words; keep the words before the cut.
                    passed = recognize(first,first+224000)
                    cut = word_gap(passed,first)
                    if cut is not None:
                        end,policy = cut,'word_gap'
                        raw = [w for w in passed if sample(w['start'],first,first+224000)+sample(w['end'],first,first+224000)<2*cut]
                if raw is None:raw = recognize(first,end)
                kept,dropped = speech(raw,first,end)
                recognition_windows.append({'start_sample':first,'end_sample':end,'end_policy':policy,
                    'first_word':len(words),'end_word':len(words)+len(kept)})
                words.extend(kept);notes.extend(dropped);first = end
        require(len(words) <= 2048, 'RESULT_LIMIT', 'Recognition returns at most 2048 words')
        gc.collect(); torch.cuda.empty_cache()
        recognized = time.monotonic()
        # Sound without speech, such as a music bed alone, gives an empty result with its notes.
        output, blocks, alignment_windows = align(pcm,words,recognition_windows,aligner(),given is not None) if words else ([],[],[])
        checked_file(str(source),(item['source_bytes'],item['source_sha256']))
        return {'profile':PROFILE if given is None else ALIGN_PROFILE,'source_sha256':item['source_sha256'],'sample_count':count,
            'words':output,'model_sha256':MODEL[1] if given is None else None,
            'recognition_seconds':recognized-item_started,'item_seconds':time.monotonic()-item_started,
            'alignment_blocks':blocks,'recognition_blocks':recognition_windows,
            'alignment_windows':alignment_windows,'non_speech':notes}

    outcomes = []
    for item in items:
        try:
            outcomes.append({'ok':True,'result':analyse(item)})
        except Failure as exc:
            if exc.code in FATAL: raise
            outcomes.append({'ok':False,'error':{'code':exc.code,'message':exc.message}})
    loaded.clear(); gc.collect(); torch.cuda.empty_cache()
    if model_path is not None: checked_file(str(model_path),MODEL)
    for name,value in ALIGNMENT[request['language']].items(): checked_file(str(files[name]),value)
    peak_cpu = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss*1024
    peak_gpu = torch.cuda.max_memory_allocated()
    require(peak_cpu <= 6*1024**3 and peak_gpu <= 8*1024**3, 'RESOURCE_LIMIT', 'Speech profile exceeded its memory gate')
    shared = {'protocol':PROTOCOL,'language':request['language'],'versions':versions,'alignment_files':ALIGNMENT[request['language']],
        'network_interfaces':interfaces,'python_network_attempts':len(network_attempts),
        'peak_cpu_bytes':peak_cpu,'peak_cuda_bytes':peak_gpu,'total_seconds':time.monotonic()-started,'items':len(items),
        'context_samples':{'leading':1280,'trailing':320},'review_required':True}
    # Each item's result carries the launch's shared facts, so it reads like a launch of its own.
    return {'protocol':PROTOCOL,'items':[{'ok':True,'result':{**shared,**o['result']}} if o['ok'] else o for o in outcomes]}


if __name__ == '__main__':
    try:
        data = sys.stdin.buffer.read(65537)
        require(len(data) <= 65536,'REQUEST_LIMIT','Worker request exceeds 64 KiB')
        value = run(json.loads(data))
        encoded = json.dumps({'ok':True,'result':value},ensure_ascii=False,allow_nan=False).encode('utf-8')
        require(len(encoded) <= RESULT_BYTES,'RESULT_LIMIT','Worker response exceeds 16 MiB')
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
