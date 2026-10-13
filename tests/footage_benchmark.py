"""Original comparative-pilot inputs and an output-only audit; no model calls or answer recipes."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import wave

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
W, H, FPS, RATE, COUNT = 320, 180, 25, 48000, 300
TASKS = ('trim', 'reorder', 'broll-inset')


def identity(path):
    with Path(path).open('rb') as stream:
        return {'bytes': Path(path).stat().st_size, 'sha256': hashlib.file_digest(stream, 'sha256').hexdigest()}


def frame(n, source):
    y, x = np.indices((H, W))
    value = np.stack(((x//8 + 30 + source*70) % 220,
                      (y//4 + 50 + n*3) % 220,
                      ((x+y)//8 + 70 + source*40) % 220), axis=-1).astype(np.uint8)
    left = (n*3) % (W-30)
    value[65:95,left:left+30] = [220,30+source*100,40]
    value[:26] = 32
    for bit in range(9):
        value[4:20,8+bit*24:24+bit*24] = 235 if n & (1<<bit) else 16
    value[4:20,284:300] = 235 if source else 16
    return value


def prepare(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents, 'Generated media belongs outside the checkout'
    root.mkdir(parents=True, exist_ok=False)
    ffmpeg = Path(shutil.which('ffmpeg')).resolve()
    ffprobe = Path(shutil.which('ffprobe')).resolve()
    manifest = {'sources': {}, 'ffmpeg': identity(ffmpeg), 'ffprobe': identity(ffprobe),
                'ffmpeg_version': subprocess.check_output([str(ffmpeg),'-version'],text=True).splitlines()[0],
                'generator': identity(__file__), 'width': W, 'height': H, 'fps': FPS, 'frames': COUNT}
    for source, name in enumerate(('a', 'b')):
        t = np.arange(COUNT*RATE//FPS,dtype=np.float64)/RATE
        signal = np.stack([np.rint(7000*np.sin(2*np.pi*((180+source*100+channel*70)*t+4*t*t)))
                           for channel in range(2)],axis=1).astype('<i2')
        wav = root/(name+'.wav')
        with wave.open(str(wav),'wb') as output:
            output.setnchannels(2); output.setsampwidth(2); output.setframerate(RATE)
            output.writeframes(signal.tobytes())
        output = root/(name+'.mov')
        codec = ['-c:v','libx264','-preset','fast','-crf','16','-pix_fmt','yuv420p'] if source==0 else [
            '-c:v','libx265','-preset','fast','-crf','16','-pix_fmt','yuv420p10le','-tag:v','hvc1',
            '-x265-params','pools=2:frame-threads=2:keyint=50:min-keyint=50:scenecut=0']
        command = [str(ffmpeg),'-v','error','-nostdin','-n','-f','rawvideo','-pix_fmt','rgb24','-s',f'{W}x{H}',
                   '-r',str(FPS),'-i','pipe:0','-i',str(wav),'-threads','2',*codec,'-color_primaries','bt709',
                   '-color_trc','bt709','-colorspace','bt709','-color_range','tv','-c:a','pcm_s16le',str(output)]
        with (root/(name+'.encoder.log')).open('xb') as log:
            process = subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=log)
            try:
                for n in range(COUNT): process.stdin.write(frame(n,source).tobytes())
                process.stdin.close()
                assert process.wait(timeout=120)==0
            finally:
                if process.poll() is None:process.kill();process.wait()
        manifest['sources'][output.name] = identity(output)
        manifest['sources'][output.name]['command'] = command
    (root/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
    return manifest


def decode(path, audio=False):
    arguments = ['-map','0:a:0','-c:a','pcm_s16le','-f','s16le'] if audio else ['-map','0:v:0','-pix_fmt','rgb24','-f','rawvideo']
    data = subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(path),*arguments,'-'],timeout=180)
    return np.frombuffer(data,dtype='<i2').reshape(-1,2) if audio else np.frombuffer(data,np.uint8).reshape(-1,H,W,3)


def assembly(inputs, task):
    a = decode(inputs/'a.mov'); audio = decode(inputs/'a.mov',True)
    ranges = [(1,4)] if task=='trim' else [(8,10),(1,3),(5,6)] if task=='reorder' else [(2,8)]
    picture = np.concatenate([a[start*FPS:end*FPS] for start,end in ranges])
    sound = np.concatenate([audio[start*RATE:end*RATE] for start,end in ranges])
    counters = [(0,n) for start,end in ranges for n in range(start*FPS,end*FPS)]
    inset = []
    if task=='broll-inset':
        b = decode(inputs/'b.mov')
        picture[FPS:3*FPS] = b[FPS:3*FPS]
        counters[FPS:3*FPS] = [(1,n) for n in range(FPS,3*FPS)]
        small = (b[4*FPS:6*FPS].astype(np.uint32).reshape(2*FPS,45,4,80,4,3).sum(axis=(2,4))//16).astype(np.uint8)
        picture[4*FPS:6*FPS,125:170,230:310] = small
        inset = [(1,n) for n in range(4*FPS,6*FPS)]
    return picture, sound, counters, inset


def counters(picture, inset=False):
    result=[]
    def block(image,left):
        # Interior means avoid chroma-edge and compression ringing; inset uses the same source cells.
        if inset:return image[125+2:125+4,230+(left+4)//4:230+(left+12)//4].mean()
        return image[8:16,left+4:left+12].mean()
    for image in picture:
        n=sum((1<<bit) for bit in range(9) if block(image,8+24*bit)>128)
        result.append((int(block(image,284)>128),n))
    return result


def audit(inputs, workspace, task):
    expected, pcm, expected_counters, expected_inset = assembly(inputs,task)
    manifest=json.loads((inputs/'manifest.json').read_text())
    preserved=all((workspace/name).is_file() and identity(workspace/name)=={k:record[k] for k in ('bytes','sha256')} for name,record in manifest['sources'].items())
    output=workspace/'delivery.mp4'
    report={'task':task,'source_preservation':preserved,'output_exists':output.is_file(),'passed':False}
    if not output.is_file():return report
    try:
        probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-show_streams','-show_format','-of','json',str(output)]))
        video=next(s for s in probe['streams'] if s['codec_type']=='video')
        audio=next(s for s in probe['streams'] if s['codec_type']=='audio')
        actual=decode(output); sound=decode(output,True)
        same_shape=actual.shape==expected.shape
        picture_ok=same_shape and counters(actual)==expected_counters
        inset_ok=not expected_inset or (same_shape and counters(actual[4*FPS:6*FPS],True)==expected_inset)
        psnr=[]
        if same_shape:
            mse=((actual.astype(np.float64)-expected)**2).mean(axis=(1,2,3))
            psnr=(10*np.log10(255**2/np.maximum(mse,1e-20))).tolist()
        corr=rms=None
        if len(sound)>=len(pcm):
            sample=sound[:len(pcm)].astype(np.float64); reference=pcm.astype(np.float64)
            corr=float(np.corrcoef(sample.ravel(),reference.ravel())[0,1])
            if not np.isfinite(corr):corr=None
            rms=float(np.sqrt(np.mean((sample-reference)**2)))
        from fractions import Fraction
        duration=Fraction(audio.get('duration_ts',-1))*Fraction(audio.get('time_base','1'))
        wanted=Fraction(len(pcm),RATE)
        audio_clock=abs(duration-wanted)<=Fraction(1,RATE)
        profile=video['codec_name']=='h264' and video.get('profile')=='High' and video['width']==W and video['height']==H and Fraction(video['avg_frame_rate'])==FPS
        profile=profile and audio['codec_name']=='aac' and audio['channels']==2 and int(audio['sample_rate'])==RATE
        color=all(video.get(key)=='bt709' for key in ('color_space','color_transfer','color_primaries'))
        report.update(frame_count=len(actual),expected_frames=len(expected),frame_counters=picture_ok,inset_counters=inset_ok,
                      profile=profile,color=color,audio_clock=audio_clock,audio_presentation_seconds=str(duration),
                      decoded_audio_samples=len(sound),audio_correlation=corr,audio_rms_error=rms,
                      frame_psnr_min=min(psnr) if psnr else None,frame_psnr_median=float(np.median(psnr)) if psnr else None,
                      frame_psnr_max=max(psnr) if psnr else None,output_identity=identity(output),
                      video_bit_rate=video.get('bit_rate'),audio_bit_rate=audio.get('bit_rate'))
        report['passed']=bool(preserved and profile and color and picture_ok and inset_ok and audio_clock and psnr and min(psnr)>=26 and corr is not None and corr>=.995 and rms<=250)
    except (ValueError,KeyError,StopIteration,subprocess.CalledProcessError) as failure:
        report['error']=str(failure)
    return report


def prompt(task, arm):
    tasks={
        'trim':"Keep source a.mov's interval [1,4) seconds, preserving matching picture and sound. The result is exactly 3 seconds.",
        'reorder':"Join source a.mov's intervals [8,10), [1,3), [5,6) seconds, in that order, with matching sound. The result is exactly 5 seconds.",
        'broll-inset':"Use source a.mov's [2,8) seconds as the primary picture and uninterrupted matching primary audio. The result is exactly 6 seconds. Replace the picture at timeline [1,3) with b.mov's [1,3). At timeline [4,6), show b.mov's [4,6) as an opaque 80 by 45 picture-in-picture at pixel position [230,125], over the primary picture. Use area/block averaging when shrinking. Keep only the uninterrupted primary audio."}
    common=("Edit the original local footage in this workspace. Both sources are 12-second, 320 by 180, 25 fps MOV files with declared BT.709 SDR and stereo 48 kHz PCM. a.mov is H.264 and b.mov is HEVC Main10. "
            +tasks[task]+" Intervals include the start and exclude the end. Deliver delivery.mp4 as H.264 High, 320 by 180, 25 fps, BT.709 SDR, software CRF 18, with stereo AAC at 48 kHz and 320 kb/s. No fades, transitions, titles or extra tail. Preserve both inputs. Inspect and correct your work as needed, then report the output and limitations. Do not install software or use network resources. Work only within this workspace. ")
    return common+("Use the installed Cutbolt plugin and its intent skills. Create a new saved project. You have the plugin's MCP and Skill tools." if arm=='cutbolt' else "Use the installed ffmpeg and ffprobe through shell commands and ordinary scripts if useful. Python is installed. You have Bash, Read and Write tools. Cutbolt and its plugin are unavailable.")


if __name__=='__main__':
    parser=argparse.ArgumentParser();sub=parser.add_subparsers(dest='action',required=True)
    p=sub.add_parser('prepare');p.add_argument('--output',type=Path,required=True)
    p=sub.add_parser('audit');p.add_argument('--inputs',type=Path,required=True);p.add_argument('--workspace',type=Path,required=True);p.add_argument('--task',choices=TASKS,required=True)
    p=sub.add_parser('prompt');p.add_argument('--task',choices=TASKS,required=True);p.add_argument('--arm',choices=['cutbolt','ffmpeg'],required=True)
    args=parser.parse_args()
    if args.action=='prepare':print(json.dumps(prepare(args.output)))
    elif args.action=='prompt':print(prompt(args.task,args.arm))
    else:print(json.dumps(audit(args.inputs,args.workspace,args.task)))
