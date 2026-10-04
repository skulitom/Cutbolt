"""Original fixture authoring and inspection through an external reference library."""
import argparse
import importlib.metadata
import json
from pathlib import Path
import sys

import opentimelineio as otio


def summarize(timeline):
    tracks = []
    for track in timeline.tracks:
        items = []
        for item in track:
            interval = track.range_of_child(item)
            value = {'schema': item.schema_name(), 'name': item.name,
                     'start': interval.start_time.to_seconds(),
                     'duration': interval.duration.to_seconds()}
            if isinstance(item, otio.schema.Clip):
                value.update(url=item.media_reference.target_url,
                             source_in=item.trimmed_range().start_time.to_seconds(),
                             available_start=item.media_reference.available_range.start_time.to_seconds())
            if isinstance(item, otio.schema.Transition):
                value.update(before=item.in_offset.to_seconds(), after=item.out_offset.to_seconds())
            items.append(value)
        tracks.append({'name': track.name, 'kind': track.kind, 'enabled': track.enabled,
                       'duration': track.duration().to_seconds(), 'items': items})
    return {'name': timeline.name, 'duration': timeline.duration().to_seconds(), 'tracks': tracks}


def create(path):
    r, tr = otio.opentime.RationalTime, otio.opentime.TimeRange

    def clip(name, source, start, count, rate=25):
        return otio.schema.Clip(name=name, media_reference=otio.schema.ExternalReference(
            target_url=f'original-{source}.mkv', available_range=tr(r(0, rate), r(40*rate/25, rate))),
            source_range=tr(r(start, rate), r(count, rate)))

    def gap(count, rate=25):
        return otio.schema.Gap(source_range=tr(r(0, rate), r(count, rate)))

    def dissolve(before, after, rate=25):
        return otio.schema.Transition(name='Asymmetric blend Ω',
            transition_type=otio.schema.TransitionTypes.SMPTE_Dissolve,
            in_offset=r(before, rate), out_offset=r(after, rate))

    video = otio.schema.Track(name='Picture Ω', kind='Video', children=[gap(2),
        clip('Left', 0, 4, 10), dissolve(2, 3), clip('Right', 1, 6, 12), gap(6)])
    overlay = otio.schema.Track(name='Overlay', kind='Video', children=[gap(11), clip('Cover', 2, 2, 3)])
    disabled = otio.schema.Track(name='Disabled', kind='Video', children=[clip('Hidden', 2, 0, 30)])
    disabled.enabled = False
    audio = otio.schema.Track(name='Sound', kind='Audio', children=[gap(3841, 48000),
        clip('Sound left', 0, 5763, 19199, 48000), dissolve(3840, 5760, 48000),
        clip('Sound right', 1, 11520, 23040, 48000), gap(11520, 48000)])
    bed = otio.schema.Track(name='Bed', kind='Audio', children=[gap(1, 48000),
        clip('Sample precision', 2, 13, 57598, 48000), gap(1, 48000)])
    timeline = otio.schema.Timeline(name='Original editorial fixture Ω', tracks=[video, overlay, disabled, audio, bed])
    otio.adapters.write_to_file(timeline, str(path), adapter_name='otio_json')
    loaded = otio.adapters.read_from_file(str(path), adapter_name='otio_json')
    assert loaded.is_equivalent_to(timeline)
    return loaded


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('mode', choices=['create', 'inspect', 'normalize'])
    parser.add_argument('path', type=Path)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    assert importlib.metadata.version('opentimelineio') == '0.18.1'
    if args.mode == 'create':
        timeline = create(args.path)
    else:
        timeline = otio.adapters.read_from_file(str(args.path), adapter_name='otio_json')
        if args.mode == 'normalize':
            assert args.output and not args.output.exists()
            otio.adapters.write_to_file(timeline, str(args.output), adapter_name='otio_json')
            assert otio.adapters.read_from_file(str(args.output), adapter_name='otio_json').is_equivalent_to(timeline)
    print(json.dumps({'library': 'OpenTimelineIO', 'version': '0.18.1', 'python': sys.version,
                      'timeline': summarize(timeline)}, ensure_ascii=False))
