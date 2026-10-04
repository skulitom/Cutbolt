"""Original local annotation/mask workflow; optional external OpenCV runtime.

One JSON request from stdin or a request file. No arbitrary expressions, scripts,
network retrieval, pretrained weights, source mutation or implicit tracking.
"""
import hashlib
import importlib.metadata
import json
import math
import os
from pathlib import Path
import struct
import sys
import tempfile
from fractions import Fraction


class Failure(Exception):
    def __init__(self, code, message):
        self.code, self.message = code, message


def require(test, code, message):
    if not test:
        raise Failure(code, message)


def fields(value, required, optional=()):
    require(isinstance(value, dict) and set(required) <= value.keys()
            and value.keys() <= set(required) | set(optional),
            'INVALID_REQUEST', 'Missing or unknown fields')


def integer(value, low, high):
    require(type(value) is int and low <= value <= high,
            'INVALID_REQUEST', f'Expected integer in {low}..{high}')
    return value


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), allow_nan=False).encode()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def no_network(event, args):
    if event.startswith('socket.'):
        raise Failure('NETWORK_DISABLED', 'Network operations are disabled in this local worker')


sys.addaudithook(no_network)


def root_path(value):
    p = Path(value)
    require(p.is_absolute() and p.is_dir(), 'INVALID_PATH', 'Root must be an existing absolute directory')
    return p.resolve()


def contained(path, root):
    path = path.resolve()
    require(path.is_relative_to(root) and path != root, 'INVALID_PATH', 'Path must remain inside its declared root')
    return path


def identity(path, root):
    data = path.read_bytes()
    return {'path': path.relative_to(root).as_posix(), 'sha256': sha(data), 'bytes': len(data)}


def read_identity(item, root, limit=64*1024*1024):
    fields(item, ('path', 'sha256', 'bytes'))
    require(isinstance(item['path'], str) and item['path'] and ':' not in item['path']
            and not Path(item['path']).is_absolute()
            and all(v not in ('', '.', '..') for v in item['path'].replace('\\', '/').split('/')),
            'INVALID_PATH', 'Media path must contain only relative normal components')
    integer(item['bytes'], 1, limit)
    require(isinstance(item['sha256'], str) and len(item['sha256']) == 64
            and all(c in '0123456789abcdef' for c in item['sha256']), 'INVALID_IDENTITY', 'Expected lowercase SHA-256')
    p = contained(root / item['path'], root)
    require(p.is_file() and p.stat().st_size == item['bytes'], 'MEDIA_CHANGED', 'Source is missing or its length changed')
    with p.open('rb') as stream:
        data = stream.read(item['bytes']+1)
    require(len(data) == item['bytes'] and sha(data) == item['sha256'], 'MEDIA_CHANGED', 'Source digest changed')
    return data


def read_json(data):
    def unique(pairs):
        result = {}
        for key, value in pairs:
            require(key not in result, 'INVALID_REQUEST', 'Duplicate JSON field')
            result[key] = value
        return result
    return json.loads(data, object_pairs_hook=unique,
                      parse_constant=lambda _: (_ for _ in ()).throw(Failure('INVALID_REQUEST', 'Nonfinite JSON number')))


def runtime():
    import cv2
    import numpy as np
    packages = []
    for name, version, license_name in [('opencv-python-headless', '4.13.0.92', 'MIT wrapper; Apache-2.0 OpenCV; external wheel notices'),
                                         ('numpy', '2.2.6', 'BSD-3-Clause; external wheel notices')]:
        dist = importlib.metadata.distribution(name)
        require(dist.version == version, 'RUNTIME_MISMATCH', f'{name} must be {version}')
        records = []
        for relative in sorted(dist.files or [], key=str):
            if str(relative).endswith(('.pyc', 'RECORD')):
                continue
            p = Path(dist.locate_file(relative))
            require(p.is_file(), 'RUNTIME_MISMATCH', 'Installed package file is missing')
            records.append([str(relative).replace('\\', '/'), sha(p.read_bytes())])
        require(records, 'RUNTIME_MISMATCH', 'Installed package inventory missing')
        packages.append({'name': name, 'version': version, 'license': license_name,
                         'files': len(records), 'tree_sha256': sha(canonical(records))})
    cv2.setNumThreads(1)
    cv2.ocl.setUseOpenCL(False)
    require(not cv2.ocl.useOpenCL() and cv2.getNumThreads() == 1, 'RUNTIME_MISMATCH', 'CPU worker setup failed')
    runtime_files = []
    for name in ('python.exe', f'python{sys.version_info.major}{sys.version_info.minor}.dll', 'vcruntime140.dll', 'vcruntime140_1.dll'):
        path = Path(sys.base_prefix)/name
        if path.is_file():
            runtime_files.append({'name':name, 'sha256':sha(path.read_bytes())})
    require(any(f['name'].startswith('python') and f['name'].endswith('.dll') for f in runtime_files),
            'RUNTIME_MISMATCH', 'Python runtime library identity unavailable')
    provenance = {'base_runtime_files':runtime_files, 'profile': 'opencv-grabcut-annotated-v1', 'packages': packages,
                  'python_version': sys.version.split()[0], 'executable_sha256': sha(Path(sys.executable).read_bytes()),
                  'helper_sha256': sha(Path(__file__).read_bytes()), 'opencv_version': cv2.__version__,
                  'threads': 1, 'opencl': False, 'network': 'Python socket operations rejected by audit hook',
                  'weights': 'No pretrained weights; per-frame foreground/background GMM fitted from this source and annotations',
                  'runtime_model_downloads': False}
    return cv2, np, provenance


def png(data, cv2, np, binary=False):
    require(data[:8] == b'\x89PNG\r\n\x1a\n' and len(data) >= 33, 'UNSUPPORTED_IMAGE', 'Expected PNG')
    width, height, depth, color = struct.unpack('>IIBB', data[16:26])
    require(1 <= width <= 512 and 1 <= height <= 512 and depth == 8 and color == 2,
            'UNSUPPORTED_IMAGE', 'Segmentation requires opaque RGB8 PNG up to 512 squared')
    position = 8
    while position < len(data):
        require(position+12 <= len(data), 'UNSUPPORTED_IMAGE', 'Truncated PNG')
        size = struct.unpack('>I', data[position:position+4])[0]
        kind = data[position+4:position+8]
        require(kind in (b'IHDR', b'IDAT', b'IEND', b'sRGB'), 'UNSUPPORTED_IMAGE', 'PNG metadata or animation is unsupported')
        position += size+12
    require(position == len(data), 'UNSUPPORTED_IMAGE', 'Truncated PNG chunk')
    image = cv2.imdecode(np.frombuffer(data, dtype=np.uint8), cv2.IMREAD_UNCHANGED)
    require(image is not None and image.shape == (height, width, 3), 'UNSUPPORTED_IMAGE', 'Invalid RGB PNG')
    if binary:
        require(bool(np.all(image[:, :, 0] == image[:, :, 1]) and np.all(image[:, :, 1] == image[:, :, 2])
                     and np.all((image[:, :, 0] == 0) | (image[:, :, 0] == 255))),
                'INVALID_MASK', 'Mask must contain only black or white RGB pixels')
    return image


def rectangle(rect, width, height):
    require(isinstance(rect, list) and len(rect) == 4, 'INVALID_ANNOTATION', 'Expected x/y/width/height rectangle')
    x, y, w, h = rect
    integer(x, 0, width-1); integer(y, 0, height-1)
    integer(w, 1, width-x); integer(h, 1, height-y)
    return slice(y, y+h), slice(x, x+w)


def annotation_mask(annotation, width, height, np):
    fields(annotation, ('region', 'foreground', 'background'))
    mask = np.zeros((height, width), dtype=np.uint8)
    mask[rectangle(annotation['region'], width, height)] = 3
    hard_fg = np.zeros_like(mask, dtype=bool)
    hard_bg = np.zeros_like(mask, dtype=bool)
    for name, target in [('foreground', hard_fg), ('background', hard_bg)]:
        regions = annotation[name]
        require(isinstance(regions, list) and len(regions) <= 256, 'LIMIT_EXCEEDED', 'At most 256 rectangles per annotation class')
        for rect in regions:
            target[rectangle(rect, width, height)] = True
    require(not bool(np.any(hard_fg & hard_bg)) and not bool(np.any(hard_fg & (mask == 0))),
            'INVALID_ANNOTATION', 'Foreground/background constraints conflict or foreground is outside the region')
    mask[hard_bg] = 0
    mask[hard_fg] = 1
    require(int(np.count_nonzero(mask == 1)) >= 5 and int(np.count_nonzero(mask == 0)) >= 5,
            'INVALID_ANNOTATION', 'At least five definite foreground and background pixels are required')
    return mask


def prepare(recipe, root, cv2, np):
    fields(recipe, ('schema_version', 'id', 'automation', 'iterations', 'seed', 'frames'))
    integer(recipe['schema_version'], 1, 1)
    require(isinstance(recipe['id'], str) and 1 <= len(recipe['id']) <= 128,
            'INVALID_REQUEST', 'Expected recipe version 1 and bounded ID')
    require(recipe['automation'] == 'annotated_frames', 'UNSUPPORTED_AUTOMATION',
            'Every frame requires explicit annotations; automatic tracking, object selection and generated annotations are unsupported')
    integer(recipe['iterations'], 1, 10); integer(recipe['seed'], 0, 2147483647)
    require(isinstance(recipe['frames'], list) and 1 <= len(recipe['frames']) <= 64,
            'LIMIT_EXCEEDED', 'Expected 1..64 annotated frames')
    prepared, total, pixels, dimensions, identities = [], Fraction(0), 0, None, {}
    for frame in recipe['frames']:
        fields(frame, ('source', 'hold', 'annotations'))
        fields(frame['hold'], ('num', 'den'))
        integer(frame['hold']['num'], 1, 10000); integer(frame['hold']['den'], 1, 10000)
        hold = Fraction(frame['hold']['num'], frame['hold']['den'])
        require((hold*25).denominator == 1, 'UNALIGNED_TIME', 'Frame holds must align to 25 fps')
        total += hold
        source = frame['source']; path = source.get('path')
        require(path not in identities or identities[path] == source, 'INVALID_IDENTITY', 'Conflicting source identities')
        identities[path] = source
        image = png(read_identity(source, root), cv2, np)
        h, w = image.shape[:2]
        if dimensions is None:
            dimensions = (w, h)
        require(dimensions == (w, h), 'UNSUPPORTED_IMAGE', 'All sequence frames must have equal dimensions')
        pixels += w*h
        require(pixels <= 4194304 and total <= 10, 'LIMIT_EXCEEDED', 'Sequence exceeds 4,194,304 pixels or ten seconds')
        mask = annotation_mask(frame['annotations'], w, h, np)
        prepared.append((image, mask))
    return prepared, dimensions, total


def model_digest(model):
    fields(model, ('kind', 'background', 'foreground', 'sha256'))
    require(model['kind'] == 'grabcut-gmm-v1', 'INVALID_MODEL', 'Unsupported fitted model type')
    values = []
    for name in ('background', 'foreground'):
        numbers = model[name]
        require(isinstance(numbers, list) and len(numbers) == 65 and all(type(x) in (float, int) and math.isfinite(x) for x in numbers),
                'INVALID_MODEL', 'Expected two finite 65-value model arrays')
        values += numbers
    digest = sha(struct.pack('<130d', *values))
    require(digest == model['sha256'], 'INVALID_MODEL', 'Fitted model digest does not match its arrays')
    return digest


def document(item, root, cv2, np):
    data = read_json(read_identity(item, root, 4*1024*1024))
    fields(data, ('schema_version', 'id', 'revision', 'parent', 'recipe', 'frames', 'provenance'))
    integer(data['schema_version'], 1, 1)
    require(data['id'] == data['recipe']['id'], 'INVALID_DOCUMENT', 'Invalid mask document version or ID')
    integer(data['revision'], 1, 1000000)
    require((data['revision'] == 1) == (data['parent'] is None), 'INVALID_DOCUMENT', 'Invalid revision parent')
    prepared, dimensions, total = prepare(data['recipe'], root, cv2, np)
    require(isinstance(data['frames'], list) and len(data['frames']) == len(prepared), 'INVALID_DOCUMENT', 'Mask/frame counts differ')
    require(isinstance(data['provenance'], dict) and data['provenance'].get('profile') == 'opencv-grabcut-annotated-v1',
            'INVALID_DOCUMENT', 'Missing declared producer provenance')
    for frame, (image, constraints) in zip(data['frames'], prepared):
        fields(frame, ('mask', 'model', 'foreground_pixels'))
        mask = png(read_identity(frame['mask'], root), cv2, np, binary=True)
        require(mask.shape == image.shape, 'INVALID_MASK', 'Mask dimensions differ')
        fg = mask[:, :, 0] == 255
        require(int(np.count_nonzero(fg)) == frame['foreground_pixels'] and bool(np.all(fg[constraints == 1]))
                and not bool(np.any(fg[constraints == 0])), 'INVALID_MASK', 'Mask violates stored annotation constraints/count')
        model_digest(frame['model'])
    return data, dimensions, total


def produce(recipe, revision, parent, request, root, cv2, np, provenance):
    integer(revision, 1, 1000000)
    prepared, dimensions, total = prepare(recipe, root, cv2, np)
    output_root = root_path(request['output_root'])
    require(Path(request['output']).is_absolute(), 'INVALID_PATH', 'Output must be absolute')
    output = contained(Path(request['output']), output_root)
    contained(output, root)
    require(output.parent.is_dir() and not output.exists(), 'OUTPUT_EXISTS', 'Output must be a new directory below both declared roots')
    scratch = Path(tempfile.mkdtemp(prefix='.cutbolt-segmentation-', dir=output.parent))
    frames = []
    try:
        for i, (image, mask) in enumerate(prepared):
            bg = np.zeros((1, 65), dtype=np.float64); fg = np.zeros((1, 65), dtype=np.float64)
            cv2.setRNGSeed((recipe['seed']+i) % 2147483648)
            cv2.grabCut(image, mask, None, bg, fg, recipe['iterations'], cv2.GC_INIT_WITH_MASK)
            binary = np.where((mask == 1) | (mask == 3), 255, 0).astype(np.uint8)
            encoded, content = cv2.imencode('.png', np.repeat(binary[:, :, None], 3, axis=2), [cv2.IMWRITE_PNG_COMPRESSION, 9])
            require(encoded, 'SEGMENTATION_FAILED', 'Mask encoding failed')
            name = f'mask-{i:03d}.png'; raw = content.tobytes()
            (scratch/name).write_bytes(raw)
            model = {'kind': 'grabcut-gmm-v1', 'background': bg[0].tolist(), 'foreground': fg[0].tolist(),
                     'sha256': sha(struct.pack('<130d', *bg[0], *fg[0]))}
            model_digest(model)
            frames.append({'mask': {'path': (output/name).relative_to(root).as_posix(), 'sha256': sha(raw), 'bytes': len(raw)},
                           'model': model, 'foreground_pixels': int(np.count_nonzero(binary))})
        doc = {'schema_version': 1, 'id': recipe['id'], 'revision': revision, 'parent': parent,
               'recipe': recipe, 'frames': frames, 'provenance': provenance}
        raw = canonical(doc)+b'\n'; (scratch/'document.json').write_bytes(raw)
        for frame in recipe['frames']:
            read_identity(frame['source'], root)
        if parent:
            document(parent, root, cv2, np)
        require(not output.exists(), 'OUTPUT_EXISTS', 'Output appeared during processing')
        # A directory hard link is unavailable. On Windows rename is atomic and refuses
        # an existing destination; this selected worker profile is Windows only.
        os.rename(scratch, output)
        return {'document': identity(output/'document.json', root), 'revision': revision,
                'dimensions': dimensions, 'duration': {'num': total.numerator, 'den': total.denominator},
                'frames': len(frames), 'provenance': provenance}
    finally:
        if scratch.exists():
            # Exact owned paths only; never enumerate or recursively remove user content.
            for i in range(len(prepared)):
                (scratch/f'mask-{i:03d}.png').unlink(missing_ok=True)
            (scratch/'document.json').unlink(missing_ok=True)
            scratch.rmdir()


def dispatch(request):
    require(isinstance(request, dict), 'INVALID_REQUEST', 'Expected object request')
    command = request.get('command')
    keys = {'segment': ('recipe', 'output_root', 'output'),
            'correct': ('document', 'expected_revision', 'corrections', 'output_root', 'output'),
            'inspect': ('document',), 'scene': ('document', 'scene_id', 'background')}
    require(command in keys, 'UNKNOWN_COMMAND', 'Expected segment, correct, inspect or scene')
    fields(request, ('command', 'input_root')+keys[command])
    require(os.name == 'nt', 'UNSUPPORTED_RUNTIME', 'This verified optional worker profile requires Windows')
    root = root_path(request['input_root'])
    cv2, np, provenance = runtime()
    if command == 'segment':
        return produce(request['recipe'], 1, None, request, root, cv2, np, provenance)
    doc, dimensions, total = document(request['document'], root, cv2, np)
    if command == 'inspect':
        return {'document': doc, 'dimensions': dimensions, 'duration': {'num': total.numerator, 'den': total.denominator},
                'current_runtime': provenance, 'runtime_matches_producer': provenance == doc['provenance']}
    if command == 'correct':
        integer(request['expected_revision'], 1, 1000000)
        require(request['expected_revision'] == doc['revision'], 'REVISION_CONFLICT', 'Mask revision changed')
        edits = request['corrections']
        require(isinstance(edits, list) and 1 <= len(edits) <= len(doc['frames']), 'INVALID_REQUEST', 'Expected bounded correction list')
        seen = set()
        for edit in edits:
            fields(edit, ('frame', 'annotations'))
            i = integer(edit['frame'], 0, len(doc['frames'])-1)
            require(i not in seen, 'INVALID_REQUEST', 'Duplicate frame correction')
            seen.add(i); doc['recipe']['frames'][i]['annotations'] = edit['annotations']
        return produce(doc['recipe'], doc['revision']+1, request['document'], request, root, cv2, np, provenance)
    require(isinstance(request['scene_id'], str) and 1 <= len(request['scene_id']) <= 128,
            'INVALID_REQUEST', 'Expected bounded scene ID')
    background = request['background']
    require(isinstance(background, list) and len(background) == 3, 'INVALID_REQUEST', 'Expected RGB background')
    for v in background:
        integer(v, 0, 255)
    duration = {'num': total.numerator, 'den': total.denominator}
    frames = [{'image': f['source'], 'matte': m['mask'], 'hold': f['hold'], 'offset': [0, 0], 'anchor': [0, 0]}
              for f, m in zip(doc['recipe']['frames'], doc['frames'])]
    scene = {'schema_version': 1, 'id': request['scene_id'], 'width': dimensions[0], 'height': dimensions[1],
             'output_scale': 1, 'duration': duration, 'background': background, 'color': 'srgb_straight_encoded',
             'audio': None, 'layers': [{'id': 'foreground', 'canvas': list(dimensions), 'start': {'num': 0, 'den': 1},
             'duration': duration, 'frames': frames, 'timing': 'strict', 'end': 'hold_last',
             'transform': {'position': [0, 0], 'crop': [0, 0, *dimensions], 'scale': 1, 'quarter_turns': 0, 'opacity': 255}}]}
    return {'scene': scene, 'mask_document': request['document'], 'mask_revision': doc['revision'],
            'scope': 'Binary held source masks; no soft matting, automatic tracking or inferred intermediate frames'}


def main():
    try:
        if len(sys.argv) == 2:
            with Path(sys.argv[1]).open('rb') as stream:
                raw = stream.read(4*1024*1024+1)
        else:
            require(len(sys.argv) == 1, 'INVALID_REQUEST', 'Pass one request file or use stdin')
            raw = sys.stdin.buffer.read(4*1024*1024+1)
        require(len(raw) <= 4*1024*1024, 'LIMIT_EXCEEDED', 'Request exceeds 4 MiB')
        result = dispatch(read_json(raw))
        print(json.dumps({'ok': True, 'result': result}, allow_nan=False))
    except Failure as exc:
        print(json.dumps({'ok': False, 'error': {'code': exc.code, 'message': exc.message}})); return 1
    except Exception as exc:
        print(json.dumps({'ok': False, 'error': {'code': 'SEGMENTATION_FAILED', 'message': str(exc)}})); return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
