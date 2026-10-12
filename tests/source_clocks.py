"""Independent normalized-clock, native-YUV and linear-PCM reference for short original footage."""
from fractions import Fraction as F
import hashlib
import json
import subprocess

import numpy as np

from engine import ENGINE
from footage_sources import FFMPEG, identity


def time(value):
    value = F(value)
    return {'num': value.numerator, 'den': value.denominator}


def call(request, code=None):
    process = subprocess.run([str(ENGINE)], input=json.dumps(request), capture_output=True,
                             text=True, encoding='utf-8', timeout=600)
    response = json.loads(process.stdout)
    if code:
        assert process.returncode != 0 and response['error']['code'] == code, response
        return response['error']
    assert process.returncode == 0 and response['ok'], response
    return response['result']


def decoded(path, video, native=False):
    fmt = native if isinstance(native,str) else 'yuv420p' if native else 'rgb24'
    args = ['-map','0:v:0','-an','-fps_mode','passthrough','-pix_fmt',fmt,'-f','rawvideo'] if video else [
        '-map','0:a:0','-vn','-c:a','pcm_s16le','-f','s16le']
    return subprocess.check_output([FFMPEG,'-v','error','-nostdin','-noautorotate','-i',str(path),*args,'pipe:1'],timeout=600)


def rgb_reference(native, width, height, full=False):
    # Independent full-frame vector arithmetic. Chroma expands in separate dimensions; no
    # engine table or converter is used. Transfer stays BT.709 on both sides, bypassing gamma.
    n = width * height
    planes = np.frombuffer(native, dtype=np.uint8).reshape(-1, n * 3 // 2)
    y = (planes[:,:n].reshape(-1,height,width).astype(np.float64) - (0 if full else 16)) / (255 if full else 219)
    def chroma(first):
        small = planes[:,first:first+n//4].reshape(-1,height//2,width//2)
        return (small.repeat(2,axis=1).repeat(2,axis=2).astype(np.float64) - 128) / (255 if full else 224)
    cb, cr = chroma(n), chroma(n+n//4)
    red, blue = y + 1.5748 * cr, y + 1.8556 * cb
    green = (y - 0.2126 * red - 0.0722 * blue) / 0.7152
    return np.floor(np.clip(np.stack((red,green,blue),axis=-1),0,1)*255+0.5).astype(np.uint8).tobytes()


def pcm_reference(raw, channels, rate, offset, count, source_in=F(0)):
    samples = np.frombuffer(raw,dtype='<i2').reshape(-1,channels).astype(np.int64)
    # offset is the exact native-sample position at the first video frame. Compute one rational
    # progression, then divide once per output sample; negative and overlong positions are silence.
    position = offset + source_in * rate
    denominator = position.denominator * 48000
    numerators = position.numerator * 48000 + np.arange(count,dtype=np.int64) * rate * position.denominator
    first, remainder = np.divmod(numerators,denominator)
    valid = (first >= 0) & (first < len(samples))
    a = np.clip(first,0,len(samples)-1)
    b = np.minimum(a+1,len(samples)-1)
    weighted = samples[a]*(denominator-remainder[:,None]) + samples[b]*remainder[:,None]
    rounded = ((np.abs(weighted)+denominator//2)//denominator)*np.sign(weighted)
    rounded[~valid] = 0
    if channels == 1:
        rounded = np.repeat(rounded,2,axis=1)
    return rounded.astype('<i2').tobytes()


def verify(root, name, case, data):
    video = next(s for s in data['streams'] if s['codec_type']=='video')
    audio = next(s for s in data['streams'] if s['codec_type']=='audio')
    frames = [f for f in data['frames'] if f['media_type']=='video']
    audio_frames = [f for f in data['frames'] if f['media_type']=='audio']
    rate, audio_rate = F(case['rate']), int(audio['sample_rate'])
    video_zero = int(frames[0]['best_effort_timestamp']) * F(video['time_base'])
    audio_zero = int(audio_frames[0]['best_effort_timestamp']) * F(audio['time_base'])
    # Container timestamps are rescaled to the decoded PCM's native sample clock before
    # alignment, exactly at nearest integer with signed half ties away from zero.
    sample_zero = audio_zero * audio_rate
    tick = (abs(sample_zero.numerator)*2 + sample_zero.denominator) // (sample_zero.denominator*2)
    audio_zero = F(tick if sample_zero >= 0 else -tick, audio_rate)
    offset = (video_zero-audio_zero) * audio_rate
    duration = len(frames) / rate
    path = root/'sources'/case['path']
    output = root/'conform'
    output.mkdir(exist_ok=True)
    recipe = {'schema_version':1,'id':name,'source':{'file':{'path':case['path'],**identity(path)},
        'sdr':{'matrix':'bt709','range':'full' if video['color_range']=='pc' else 'limited','transfer':'bt709','missing_tags':'reject'},
        'clock':'first_video_frame'},'source_in':time(0),'duration':time(duration),'rate':time(1),
        'reverse':False,'freeze':False,'width':case['size'][0],'height':case['size'][1],
        'audio':'resample','working_transfer':'bt709','frame_rate':time(rate)}
    request = {'command':'media.conform','recipe':recipe,'input_root':str(root/'sources'),
               'output_root':str(output),'output':str(output/(name+'.mkv'))}
    receipt = call(request)
    assert receipt['clock']['video_first_timestamp'] == time(video_zero), receipt['clock']
    assert receipt['clock']['audio_sample_offset_at_video_zero'] == time(offset), receipt['clock']
    assert receipt['source_frame_indices'] == list(range(len(frames))), name
    native = decoded(path,True,video['pix_fmt'])
    expected_rgb = rgb_reference(native,*case['size'],full=video['color_range']=='pc')
    expected_pcm = pcm_reference(decoded(path,False),case['channels'],audio_rate,offset,int(duration*48000))
    actual_rgb, actual_pcm = decoded(request['output'],True), decoded(request['output'],False)
    assert actual_rgb == expected_rgb, (name,'RGB mismatch')
    assert actual_pcm == expected_pcm, (name,'PCM mismatch',len(actual_pcm),len(expected_pcm))
    # Legacy origin checks still reject the offset fixture without explicit normalization.
    if name == 'h264-edit-list':
        legacy = {**recipe,'source':{k:v for k,v in recipe['source'].items() if k!='clock'}}
        call({'command':'media.conform.inspect','recipe':legacy,'input_root':str(root/'sources')},'UNSUPPORTED_MEDIA')
    assert identity(path) == {k:case[k] for k in ('sha256','bytes')}
    return {'frames':len(frames),'samples':len(actual_pcm)//4,'video_origin':time(video_zero),
            'audio_sample_offset':time(offset),'rgb_sha256':hashlib.sha256(actual_rgb).hexdigest(),
            'pcm_sha256':hashlib.sha256(actual_pcm).hexdigest(),'sources_preserved':True}
