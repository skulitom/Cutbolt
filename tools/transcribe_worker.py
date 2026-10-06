"""Original optional offline speech adapter; all recognizer/model code stays external.

The fixed profile returns estimated acoustic evidence and explicit contextual cut
intervals. Nothing here authenticates speech or substitutes for reviewing a cut.
Run only through transcribe_supervisor.py in the selected Linux namespaces.
"""
from bisect import bisect_left
import gc
import hashlib
import importlib.metadata as metadata
from itertools import count as numbering
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


def prompt_of(vocabulary):
    """The recognizer prompt of a vocabulary: its terms as one comma-separated line."""
    return ', '.join(vocabulary)+'.' if vocabulary else None


def core(text):
    """A token's letters and digits, case-folded, so "Forge," and "forge" compare equal."""
    return ''.join(c for c in text.casefold() if c.isalnum())


def respell(words, vocabulary):
    """Respell recognized tokens as the vocabulary's one-word terms.

    Two to four consecutive tokens whose letters and digits run together into a term become that
    term ("pixel forge" for PixelForge), keeping the first token's leading and the last token's
    trailing punctuation; tokens with punctuation between them are not joined. A single token
    spelled differently takes the term's spelling, except that an all-lowercase term ("um") leaves
    a token that differs only by a capital first letter. Returns the words and the count changed.
    """
    terms = {}
    for term in vocabulary or []:
        if len(term.split()) == 1 and core(term):
            terms.setdefault(core(term), term)
    output, i, changed = [], 0, 0
    while i < len(words):
        for n in (4, 3, 2, 1):
            group = words[i:i+n]
            texts = [w['word'] for w in group]
            term = terms.get(''.join(core(t) for t in texts)) if len(group) == n else None
            if term is None or any(not t[-1].isalnum() for t in texts[:-1]) or any(not t[0].isalnum() for t in texts[1:]):
                continue
            first = next(k for k,c in enumerate(texts[0]) if c.isalnum())
            last = next(k for k,c in reversed(list(enumerate(texts[-1]))) if c.isalnum())
            lead, trail = texts[0][:first], texts[-1][last+1:]
            spelled = texts[0][first:last+1] if n == 1 else None
            if n == 1 and (spelled == term or (term == term.lower() and spelled[1:] == term[1:])):
                output.append(group[0])
            else:
                output.append({**group[0],'word':lead+term+trail,'end':group[-1]['end'],
                    'probability':min(w['probability'] for w in group)})
                changed += 1
            i += n
            break
        else:
            output.append(words[i]); i += 1
    return output, changed


def reading(labels, letter, separator, names):
    """The letters of a greedy acoustic label path: repeats collapse, blanks drop, and word
    separators become spaces; at most 64 characters, the 64th then being an ellipsis."""
    text, previous = [], None
    for label in labels:
        if label != previous:
            if letter[label]: text.append(names[label])
            elif label == separator: text.append(' ')
        previous = label
    text = ' '.join(''.join(text).split())
    return text if len(text) <= 64 else text[:63]+'…'


def grow(rms, lo, hi, low, high, threshold):
    """Extend 5 ms frames [lo, hi) over frames at or above threshold, across holes of at most four
    frames, within [low, high)."""
    while lo > low:
        j = lo-1
        while j >= low and lo-j <= 4 and rms[j] < threshold: j -= 1
        if j < low or rms[j] < threshold: break
        lo = j
    while hi < high:
        j = hi
        while j < high and j-hi < 4 and rms[j] < threshold: j += 1
        if j >= high or rms[j] < threshold: break
        hi = j+1
    return lo, hi


# The share of frames that must hear speech: of a recognized segment's CTC spans, and of an uncovered
# sound from its first letter to its last. In the effects reel's speech check, "Thanks for watching!"
# written over a music bed that the acoustic model reads as a few scattered letters a second ("OTUR",
# "ORS OR") had 6.4% of its frames heard, and the bed's letters 7%; every narrated sentence had
# about 39% or more, every narrated word at least 20%.
HEARD_SHARE = 0.15


def uncovered(pcm, best, letter, separator, names, frames, raw, np):
    """Speech that no word covers, for review, such as a filler the recognizer left out.

    Frames whose most likely acoustic label is a letter, more than two frames from every word's
    CTC span, form groups when at most five frames apart. Each group grows over the voiced audio
    around it (5 ms RMS within 14 dB of its peak, holes up to 20 ms); groups that end up
    overlapping merge. A group whose sound runs on into a word's CTC span without such a dip is
    left to that word: it cannot be told from the word's own onset or ending, which the aligner
    can place a little late or early. A group whose peak is more than 14 dB below the median
    level inside the words is background and left out too, and so is a sound whose letters are
    scattered: fewer than HEARD_SHARE of its frames from the first letter to the last hear speech,
    as over a music bed. Each reports the letters the acoustic model read there, never a word.
    """
    count = len(pcm)
    covered = np.zeros(len(best), dtype=bool)
    for a, b in frames: covered[max(0,a-2):b+2] = True
    found = np.flatnonzero(letter[best] & ~covered)
    if not len(found) or not raw: return []
    rms = energy(pcm, np)
    level = float(np.median(np.concatenate([rms[a//80:(b+79)//80] for a, b in raw])))
    groups = []
    for f in found.tolist():
        if groups and f-groups[-1][1] < 5 and not covered[groups[-1][1]:f].any(): groups[-1][1] = f+1
        else: groups.append([f, f+1])
    output = []
    for g0, g1 in groups:
        start, end = g0*320, min(count, (g1-1)*320+400)
        low = max([b for a, b in raw if b <= start], default=None)
        high = min([a for a, b in raw if a >= end], default=None)
        floor, ceiling = (0 if low is None else (low+79)//80), (count//80 if high is None else high//80)
        lo, hi = start//80, (end+79)//80
        peak = float(rms[lo:hi].max())
        if peak < .2*level: continue
        lo, hi = grow(rms, lo, hi, floor, ceiling, max(.002, peak*.2))
        if (low is not None and lo <= floor) or (high is not None and hi >= ceiling): continue
        item = {'start_sample':lo*80,'end_sample':min(count, hi*80),'letters':reading(best[g0:g1], letter, separator, names)}
        if output and item['start_sample'] < output[-1]['end_sample']:
            first = output[-1]['first_frame']
            output[-1] = {**output[-1],'end_sample':max(output[-1]['end_sample'], item['end_sample']),
                'letters':reading(best[first:g1], letter, separator, names),'end_frame':g1}
        else:
            output.append({**item,'first_frame':g0,'end_frame':g1})
    # Speech reads as letters close together. Scattered letters merged over one continuous sound,
    # such as a music bed, are kept only when HEARD_SHARE of the frames from the first to the last
    # hear speech, as recognized segments are.
    heard = letter[best] if separator is None else letter[best] | (best == separator)
    return [{k:v for k,v in item.items() if k not in ('first_frame','end_frame')} for item in output
        if heard[item['first_frame']:item['end_frame']].mean() >= HEARD_SHARE]


def letters(text, vocab):
    """Acoustic labels of a word: its letters and apostrophes in upper case. A letter
    the vocabulary lacks falls back to its base letter (É to E) when that one exists."""
    output = []
    for c in unicodedata.normalize('NFC',text.upper()):
        if not (c.isalpha() or c == "'"): continue
        base = unicodedata.normalize('NFD',c)[0]
        output.append(c if c in vocab or base not in vocab else base)
    return ''.join(output)


ONES = ('zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen '
    'seventeen eighteen nineteen').split()
TENS = '- - twenty thirty forty fifty sixty seventy eighty ninety'.split()
SCALES = ((10**12,'trillion'),(10**9,'billion'),(10**6,'million'),(1000,'thousand'))
ORDINALS = {'one':'first','two':'second','three':'third','five':'fifth','eight':'eighth','nine':'ninth','twelve':'twelfth'}
CURRENCIES = {'$':('dollar','dollars','cent','cents'),'£':('pound','pounds','penny','pence'),'€':('euro','euros','cent','cents')}
DIGITS = frozenset('0123456789')
APOSTROPHES = "'’"


def cardinal(n):
    """English words of 0 <= n < 10**15, without "and": 170 is one hundred seventy."""
    if n < 20: return [ONES[n]]
    if n < 100: return [TENS[n//10]]+([ONES[n%10]] if n%10 else [])
    if n < 1000: return [ONES[n//100],'hundred']+(cardinal(n%100) if n%100 else [])
    value, name = next(s for s in SCALES if n >= s[0])
    return cardinal(n//value)+[name]+(cardinal(n%value) if n%value else [])


def year(n):
    """Four digits read in pairs: nineteen ninety, nineteen oh five, nineteen hundred, twenty twenty six."""
    high, low = divmod(n, 100)
    return cardinal(high)+(['hundred'] if not low else ['oh',ONES[low]] if low < 10 else cardinal(low))


def numeral(text, i):
    """The number written from text[i], a digit: its words without any decimal fraction, its whole
    value, the fraction's digits and the index after it."""
    j = i
    while j < len(text) and text[j] in DIGITS: j += 1
    digits, grouped, fraction, minutes, suffix = text[i:j], False, '', None, None
    group = lambda k: len(text[k:k+3]) == 3 and all(c in DIGITS for c in text[k:k+3]) and text[k+3:k+4] not in DIGITS
    while len(digits) <= 3 or grouped:
        if not (text[j:j+1] == ',' and group(j+1)): break
        digits, j, grouped = digits+text[j+1:j+4], j+4, True
    if text[j:j+1] == '.' and text[j+1:j+2] in DIGITS:
        k = j+1
        while k < len(text) and text[k] in DIGITS: k += 1
        fraction, j = text[j+1:k], k
    elif not grouped and len(digits) <= 2 and text[j:j+1] == ':' and len(text[j+1:j+3]) == 2 and all(c in DIGITS for c in text[j+1:j+3]) \
            and text[j+3:j+4] not in DIGITS:
        minutes, j = int(text[j+1:j+3]), j+3
    elif text[j:j+2].lower() in ('st','nd','rd','th') and not text[j+2:j+3].isalpha():
        suffix, j = 'ordinal', j+2
    elif text[j:j+1] == 's' and not text[j+1:j+2].isalpha():
        suffix, j = 'plural', j+1
    elif text[j:j+1] in APOSTROPHES and text[j+1:j+2] == 's' and not text[j+2:j+3].isalpha():
        suffix, j = 'plural', j+2
    value = int(digits)
    if (len(digits) > 1 and digits[0] == '0') or len(digits) > 15: words = [ONES[int(c)] for c in digits]
    elif not grouped and suffix != 'ordinal' and len(digits) == 4 and (1100 <= value <= 1999 or 2010 <= value <= 2099): words = year(value)
    else: words = cardinal(value)
    if minutes is not None: words += ["o'clock"] if not minutes else ['oh',ONES[minutes]] if minutes < 10 else cardinal(minutes)
    if suffix:
        last = words[-1]
        if suffix == 'ordinal': last = ORDINALS.get(last) or (last[:-1]+'ieth' if last.endswith('y') else last+'th')
        else: last = last[:-1]+'ies' if last.endswith('y') else last+'es' if last.endswith('x') else last+'s'
        words[-1] = last
    return words, value, fraction, j


def spoken(text):
    """The words a token is read as for alignment. A token without digits is read as written.

    Numbers are read in English: 80 eighty, 170 one hundred seventy, 1,500 one thousand five hundred,
    3.5 three point five, 21st twenty first, 1990s nineteen nineties, 9:05 nine oh five, $5 five
    dollars, $3.50 three dollars fifty cents, 50% fifty percent, -4 minus four. Four digits from 1100 to 1999 and 2010 to 2099 are read
    as a year (1990 nineteen ninety, 2026 twenty twenty six); a leading zero or more than 15 digits
    are read digit by digit. The letters around a number are read as their own words, so 10-second
    is ten second and mp4 is mp four.
    """
    if not any(c in DIGITS for c in text): return [text]
    words, start, i = [], 0, 0
    def letters_of(run):
        for part in ''.join(c if c.isalpha() or c in APOSTROPHES else ' ' for c in run).split():
            if part.strip(APOSTROPHES): words.append(part.strip(APOSTROPHES))
    while i < len(text):
        if text[i] not in DIGITS:
            i += 1; continue
        k = i
        currency = CURRENCIES.get(text[k-1]) if k else None
        k -= bool(currency)
        sign = k and text[k-1] in '+-−' and (k == 1 or not text[k-2].isalnum())
        letters_of(text[start:k-sign])
        if sign: words.append('plus' if text[k-1] == '+' else 'minus')
        read, value, fraction, i = numeral(text, i)
        if currency and len(fraction) == 2:
            cents = int(fraction)
            read = (read+[currency[value != 1]] if value or not cents else [])+(cardinal(cents)+[currency[2+(cents != 1)]] if cents else [])
        else:
            read += ['point']+[ONES[int(c)] for c in fraction] if fraction else []
            if currency: read.append(currency[value != 1 or bool(fraction)])
        words.extend(read)
        if text[i:i+1] == '%': words.append('percent'); i += 1
        start = i
    letters_of(text[start:])
    return words


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


def frame_at(n, frames):
    """The first 20 ms acoustic frame whose receptive field is centred at or after analysis sample n,
    at most `frames`; the frames of samples [a, b) are frame_at(a) to frame_at(b)."""
    return min(frames, max(0, (n-200+319)//320))


def emissions(pcm, loaded):
    """The acoustic model's label log-probabilities in every frame of the analysis, on its exact
    convolution clock, and the blocks they were computed in. Inputs up to 30 s are one block;
    longer ones are 24 s kept tiles with 1 s of context on either side."""
    import torch
    aligner, extractor, vocab, config = loaded
    stride, receptive = 1, 1
    for kernel, step in zip(config.conv_kernel,config.conv_stride):
        receptive += (kernel-1)*stride; stride *= step
    require((stride,receptive) == (320,400), 'MODEL_FORMAT', 'Unexpected acoustic convolution clock')
    count = len(pcm); frame_count = (count-receptive)//stride+1
    kept_tiles, blocks = [], []
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
        kept_tiles.append(kept); blocks.append({'start_sample':low,'end_sample':high,'first_frame':first,'end_frame':end})
        del logits, inputs
    return torch.cat(kept_tiles,dim=1), blocks


def unheard(words, windows, frames, heard):
    """Drop recognized segments the acoustic model hears too little of.

    Over music or noise alone a prompted recognizer can still write text: the vocabulary prompt
    itself ("Pip, PixelForge, Cutbolt.") or a stock phrase ("Thanks for watching!"). A frame hears
    speech when its most likely acoustic label is not the blank. A segment, as the recognizer
    returned it, is dropped when fewer than HEARD_SHARE of the frames of its words' CTC spans hear
    speech. Returns the kept words, the windows' word indices renumbered to them, and one
    `unheard` note per dropped segment with its words and the recognizer's times.
    """
    spans, voiced = {}, {}
    for w, (a, b) in zip(words, frames):
        spans[w['segment']] = spans.get(w['segment'], 0)+b-a
        voiced[w['segment']] = voiced.get(w['segment'], 0)+int(heard[a:b].sum())
    spoken = {s for s in spans if voiced[s] and voiced[s] >= HEARD_SHARE*spans[s]}
    keep = [i for i, w in enumerate(words) if w['segment'] in spoken]
    notes = []
    for w in words:
        if w['segment'] in spoken: continue
        if notes and notes[-1][0] == w['segment']:
            notes[-1][1].append(w)
        else:
            notes.append((w['segment'], [w]))
    windows = [{**w, 'first_word':bisect_left(keep, w['first_word']), 'end_word':bisect_left(keep, w['end_word'])} for w in windows]
    return [words[i] for i in keep], windows, [{'text':' '.join(w['word'] for w in dropped),'kind':'unheard',
        'start_sample':dropped[0]['at'][0],'end_sample':max(w['at'][1] for w in dropped)} for _, dropped in notes]


def labels_of(text, vocab, language, given):
    """The acoustic labels of one word. English numerals are aligned as the words they are read as
    (80 as EIGHTY); several spoken words are separated by the word separator. The Greek profile does
    not read numbers out: Greek number words agree with the noun they count, so digits reject."""
    origin = 'Text' if given else 'Recognized'
    sign = next((c for c in text if c.isnumeric() and c not in DIGITS), None)
    require(sign is None, 'UNSUPPORTED_ALIGNMENT_TEXT', f'{origin} word {text[:64]!r} uses {sign!r}, which this acoustic profile does not read out; write it in words')
    require(language == 'en' or not any(c in DIGITS for c in text), 'UNSUPPORTED_ALIGNMENT_TEXT',
        f'{origin} word {text[:64]!r} uses digits; the Greek profile does not read numbers out, so '
        +('write the number in Greek words' if given else 'align known text with the number written in Greek words'))
    output = []
    for part in spoken(text):
        clean = letters(part,vocab)
        require(clean and all(c in vocab for c in clean), 'UNSUPPORTED_ALIGNMENT_TEXT', f'{origin} word {text[:64]!r} has letters this acoustic profile cannot align; correct the text')
        if output: output.append(vocab['|'])
        output.extend(vocab[c] for c in clean)
    return output


def align(pcm, words, windows, loaded, given, language, emission):
    """Acoustic word intervals: CTC alignment of each window's words inside that window. Also returns
    each word's CTC span in acoustic frames."""
    import numpy as np
    import torch
    import torchaudio
    aligner, extractor, vocab, config = loaded
    labels, ranges = [], []
    for word in words:
        text = word['word']
        require(type(text) is str and len(text.encode('utf-8')) <= 512 and len(text.split()) == 1 and not any(unicodedata.category(c) == 'Cc' for c in text), 'INVALID_ALIGNMENT', 'Invalid recognized word text')
        clean = labels_of(text,vocab,language,given)
        if labels: labels.append(vocab['|'])
        first = len(labels); labels.extend(clean); ranges.append((first,len(labels)))
    require(len(labels) <= 16384, 'RESULT_LIMIT', 'Acoustic target exceeds 16384 labels')
    stride, receptive = 320, 400
    count = len(pcm); frame_count = emission.shape[1]
    repeated = sum(a == b for a,b in zip(labels,labels[1:]))
    require(len(labels)+repeated <= frame_count, 'INVALID_ALIGNMENT', 'Too many acoustic labels for this source interval')
    raw, confidences, alignment_windows, frames = [], [], [], []
    # Keep acoustic words in the disjoint source window that actually produced
    # them. Repeated text must not allow a global CTC path to shift a recognized
    # phrase into another window after the recognizer omits a word or phrase.
    for window in windows:
        word_first,word_end=window['first_word'],window['end_word']
        if word_first==word_end:continue
        label_first,label_end=ranges[word_first][0],ranges[word_end-1][1]
        target=labels[label_first:label_end]
        first,end=frame_at(window['start_sample'],frame_count),frame_at(window['end_sample'],frame_count)
        repeated=sum(a==b for a,b in zip(target,target[1:]))
        require(len(target)+repeated<=end-first,'INVALID_ALIGNMENT','Recognized window has too many labels for its acoustic clock')
        alignment,scores = torchaudio.functional.forced_align(emission[:,first:end],torch.tensor([target]),blank=config.pad_token_id)
        spans = torchaudio.functional.merge_tokens(alignment[0],scores[0].exp(),blank=config.pad_token_id)
        require([s.token for s in spans] == target, 'INVALID_ALIGNMENT', 'Acoustic path did not cover every label')
        alignment_windows.append({'first_frame':first,'end_frame':end,'first_word':word_first,'end_word':word_end})
        for i in range(word_first,word_end):
            a,b=ranges[i];selected=spans[a-label_first:b-label_first]
            frames.append((first+selected[0].start,first+selected[-1].end))
            raw.append([max(window['start_sample'],(first+selected[0].start)*stride),
                min(window['end_sample'],(first+selected[-1].end-1)*stride+receptive)])
            confidences.append(milli(sum(s.score*(s.end-s.start) for s in selected)/sum(s.end-s.start for s in selected)))
    names = {label:name for name,label in vocab.items()}
    letter = np.zeros(emission.shape[-1],dtype=bool)
    for name,label in vocab.items():
        if len(name) == 1 and (name.isalpha() or name == "'") and label < len(letter): letter[label] = True
    # Speech no word covers is reported beside the words; it leaves their intervals as they are.
    found = uncovered(pcm,emission[0].argmax(-1).numpy(),letter,vocab.get('|'),names,frames,raw,np)
    acoustic = acoustic_edges(pcm,raw,np)
    context = contextual(acoustic,count)
    output = [{'id':f'w{i:04}','text':word['word'],'start_sample':a,'end_sample':b,
        'probability_milli':None if word['probability'] is None else milli(word['probability']),'ctc_start_sample':raw[i][0],'ctc_end_sample':raw[i][1],
        'acoustic_start_sample':acoustic[i][0],'acoustic_end_sample':acoustic[i][1],'alignment_score_milli':confidences[i]}
        for i,(word,(a,b)) in enumerate(zip(words,context))]
    return output, alignment_windows, found, frames


def checked_item(item):
    fields(item, ['source_path','source_sha256','source_bytes','sample_count','text','vocabulary'])
    given, vocabulary = item['text'], item['vocabulary']
    require(given is None or (type(given) is list and 0 < len(given) <= 2048 and all(type(t) is str and len(t.encode('utf-8')) <= 512
        and len(t.split()) == 1 and t == t.strip() for t in given)), 'INVALID_REQUEST', 'Text to align must be 1..2048 single words')
    require(vocabulary is None or (given is None and type(vocabulary) is list and 0 < len(vocabulary) <= 32
        and all(type(t) is str and 0 < len(t.encode('utf-8')) <= 64 and t == t.strip()
            and not any(unicodedata.category(c) == 'Cc' for c in t) for t in vocabulary)),
        'INVALID_REQUEST', 'A vocabulary is 1..32 terms of 1..64 bytes and guides recognition only')
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
        words=[];notes=[];recognition_windows=[];respelled=0
        # The acoustic model's reading of the whole analysis: alignment uses it, and recognition
        # decodes only where it hears speech (a frame whose most likely label is not the blank).
        emission, blocks = emissions(pcm,aligner())
        heard = emission[0].argmax(-1).numpy() != aligner()[3].pad_token_id
        if given is not None:
            # Known text (a narration script): no recognition, one window over the whole analysis.
            words=[{'word':text,'probability':None} for text in given]
            recognition_windows.append({'start_sample':0,'end_sample':count,'end_policy':'given_text','first_word':0,'end_word':len(words)})
        else:
            model = recognizer()
            # The vocabulary prompts every decode of every window; the prompt must fit the
            # recognizer's context whole, or its first terms would be dropped unseen.
            prompt = prompt_of(item['vocabulary'])
            if prompt is not None:
                from whisper.tokenizer import get_tokenizer
                tokens = get_tokenizer(model.is_multilingual,num_languages=model.num_languages,language=request['language'],task='transcribe').encode(' '+prompt)
                require(len(tokens) <= model.dims.n_text_ctx//2-1, 'INPUT_LIMIT', f'The vocabulary takes {len(tokens)} recognizer tokens; at most {model.dims.n_text_ctx//2-1} fit')
            segments = numbering()
            def recognize(first,end):
                """Recognized words of [first, end), each with its segment, and whether it was decoded.
                Without speech evidence a prompted recognizer writes the prompt itself or a stock phrase
                over music alone, so a span the acoustic model hears nothing in is not decoded."""
                if not np.any(pcm[first:end]) or not heard[frame_at(first,len(heard)):frame_at(end,len(heard))].any():
                    return [],False
                result = model.transcribe(pcm[first:end],language=request['language'],task='transcribe',word_timestamps=True,
                    fp16=True,temperature=0.,beam_size=5,condition_on_previous_text=False,verbose=None,
                    initial_prompt=prompt,carry_initial_prompt=prompt is not None)
                return [{**word,'segment':n} for segment,n in zip(result['segments'],segments) for word in segment['words']],True
            rms = energy(pcm,np) if count>480000 else None
            first = 0
            while first<count:
                end,policy = (count,'source_end') if count<=480000 or count-first<=240000 else quiet_end(rms,first)
                raw = None
                if policy=='hard_12s':
                    # Recognize 14s, then cut between words; keep the words before the cut.
                    passed,decoded = recognize(first,first+224000)
                    cut = word_gap(passed,first)
                    if cut is not None:
                        end,policy = cut,'word_gap'
                        raw = [w for w in passed if sample(w['start'],first,first+224000)+sample(w['end'],first,first+224000)<2*cut]
                if raw is None:raw,decoded = recognize(first,end)
                kept,dropped = speech(raw,first,end)
                kept,changed = respell(kept,item['vocabulary']);respelled += changed
                kept = [{**w,'at':(sample(w['start'],first,end),sample(w['end'],first,end))} for w in kept]
                recognition_windows.append({'start_sample':first,'end_sample':end,'end_policy':policy,'decoded':decoded,
                    'first_word':len(words),'end_word':len(words)+len(kept)})
                words.extend(kept);notes.extend(dropped);first = end
        require(len(words) <= 2048, 'RESULT_LIMIT', 'Recognition returns at most 2048 words')
        gc.collect(); torch.cuda.empty_cache()
        recognized = time.monotonic()
        # Sound without speech, such as a music bed alone, gives an empty result.
        output, alignment_windows, found, frames = align(pcm,words,recognition_windows,aligner(),given is not None,request['language'],emission) if words else ([],[],[],[])
        if given is None and words:
            # Segments the acoustic model hears nothing of are dropped and the rest aligned again.
            words, recognition_windows, written = unheard(words,recognition_windows,frames,heard)
            if written:
                notes = sorted(notes+written,key=lambda note:note['start_sample'])
                output, alignment_windows, found, frames = align(pcm,words,recognition_windows,aligner(),False,request['language'],emission) if words else ([],[],[],[])
        checked_file(str(source),(item['source_bytes'],item['source_sha256']))
        return {'profile':PROFILE if given is None else ALIGN_PROFILE,'source_sha256':item['source_sha256'],'sample_count':count,
            'words':output,'model_sha256':MODEL[1] if given is None else None,
            'recognition_seconds':recognized-item_started,'item_seconds':time.monotonic()-item_started,
            'alignment_blocks':blocks,'recognition_blocks':recognition_windows,
            'alignment_windows':alignment_windows,'non_speech':notes,'uncovered':found,
            'vocabulary':item['vocabulary'],'respelled':respelled}

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
