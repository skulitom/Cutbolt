"""Delivery quality: what a build's checks found that a person should see before approving the delivery.

A build succeeds when every stage ran, but its checks can still find problems: a speech check that could not
listen, a delivered file louder than its peak target, a music bed that stops before the film. Each finding is a
warning, `{"code", "stage", "message", "detail"}`. Warnings are derived on every build from the stage results
and the review folders, so a reused stage still reports what it found, and a changed threshold applies without
running anything again.
"""

# Narration rate for the length estimate at `check` time, in words per second. Qwen3-TTS CustomVoice `ryan`
# measured about 2.6 words/s on the progress demos; other speakers use the same figure until measured.
WORDS_PER_SECOND = {"ryan": 2.6}
DEFAULT_WORDS_PER_SECOND = 2.6
# An estimated film within this fraction of the music bed's length counts as possibly longer than the bed.
ESTIMATE_MARGIN = 0.1
# Delivered integrated loudness may differ from the target by this much before it is reported.
LOUDNESS_TOLERANCE_LU = 1.0
# Listed speech differences per warning; the review folder has them all.
LISTED = 5


def warning(code, stage, message, **detail):
    return {"code": code, "stage": stage, "message": message, "detail": detail}


def words_per_second(voice):
    speaker = (voice or {}).get("speaker", "")
    return WORDS_PER_SECOND.get(str(speaker).lower(), DEFAULT_WORDS_PER_SECOND), str(speaker).lower() in WORDS_PER_SECOND


def music_warnings(stage, music_seconds, film_seconds, loop, estimated=False, basis=None):
    """A bed that stops before the film ends: from an estimate at `check` time, or measured once timing is known."""
    if loop != "none" or music_seconds is None or film_seconds is None:
        return []
    fix = 'supply a longer bed or set music.loop to "bars"'
    if estimated:
        if music_seconds >= film_seconds * (1 + ESTIMATE_MARGIN):
            return []
        gap = film_seconds - music_seconds
        where = (f"would end about {gap:.1f} s before it" if gap > 0 else
                 f"is within {ESTIMATE_MARGIN:.0%} of that estimate and may end before the film")
        return [warning("MUSIC_MAY_END_EARLY", "check",
                        f"the music bed lasts {music_seconds:.2f} s and the film is estimated at {film_seconds:.1f} s "
                        f"({basis}); the bed {where}; {fix}",
                        music_seconds=round(music_seconds, 3), estimated_film_seconds=round(film_seconds, 2), basis=basis)]
    if music_seconds >= film_seconds:
        return []
    return [warning("MUSIC_ENDS_EARLY", stage,
                    f"the music bed ({music_seconds:.2f} s) ends {film_seconds - music_seconds:.2f} s before the film "
                    f"({film_seconds:.2f} s), which plays the rest without music; {fix}",
                    music_seconds=round(music_seconds, 3), film_seconds=round(film_seconds, 3))]


def speech_warnings(enabled, narrated, review, min_match, stage="speech-review"):
    """A speech check that was turned off, did not run, failed, or heard too few of the expected words.
    `review` is the speech check's review.json, or None when it did not run."""
    if not narrated:
        return []
    if not enabled:
        return [warning("SPEECH_CHECK_SKIPPED", stage, "delivery.review.speech is false, so nobody listened to the delivered "
                        "narration; the words were not compared with the script")]
    if review is None:
        return [warning("SPEECH_CHECK_SKIPPED", stage, "the speech check did not run; the words were not compared with the script")]
    speech = review.get("speech") or {}
    recognition = speech.get("recognition") or {}
    if recognition.get("ok") is False:
        error = recognition.get("error") or {}
        return [warning("SPEECH_CHECK_FAILED", stage,
                        f"the speech check failed: recognition stopped with {error.get('code', 'an error')}: "
                        f"{str(error.get('message', '')).strip()[:300]}; the narration was not compared with the script",
                        error=error)]
    comparison = speech.get("comparison") or {}
    ratio = comparison.get("match_ratio")
    if ratio is None:
        return [warning("SPEECH_CHECK_INCOMPLETE", stage, "the speech check compared no words: "
                        + ((review.get("summary") or "").strip().splitlines() or ["no speech section"])[-1])]
    out = []
    if ratio < min_match:
        listed = (comparison.get("differences") or {}).get("listed") or []
        shown = "; ".join(describe(d) for d in listed[:LISTED])
        count = (comparison.get("differences") or {}).get("count", len(listed))
        out.append(warning("SPEECH_MISMATCH", stage,
                           f"the speech check heard {comparison.get('matched')} of {comparison.get('expected_words')} expected words "
                           f"({ratio:.1%}), below delivery.review.min_speech_match {min_match:.0%}; {count} difference(s)"
                           + (f": {shown}" if shown else ""),
                           match_ratio=ratio, matched=comparison.get("matched"), expected_words=comparison.get("expected_words"),
                           differences=listed[:LISTED]))
    uncovered = speech.get("uncovered") or {}
    if uncovered.get("count"):
        listed = uncovered.get("listed") or []
        shown = ", ".join(f"\"{u.get('letters', '')}\" at {seconds(u.get('start')):.2f} s" for u in listed[:LISTED])
        out.append(warning("UNCOVERED_SPEECH", stage, f"the speech check heard {uncovered['count']} sound(s) no word covers "
                           f"(a garbled or extra word in a take, or a word recognition missed): {shown}", listed=listed[:LISTED]))
    return out


def review_warnings(review, target_lkfs, peak_dbfs, music, codec=None, stage="review"):
    """Findings in the delivered file's review.json: black picture, clipping, silence under a music bed, timing that
    differs from the project, words cut by clip edges, loudness off target, and a true peak above the target."""
    out = []
    picture = review.get("picture") or {}
    black = (picture.get("black") or {}).get("count") or 0
    if black:
        runs = ", ".join(span(r) for r in (picture["black"].get("runs") or [])[:LISTED])
        out.append(warning("BLACK_PICTURE", stage, f"the delivered picture has {black} black run(s): {runs}", count=black))
    sound = review.get("sound") or {}
    over_time = sound.get("over_time") or {}
    clipping = over_time.get("clipping") or {}
    if clipping.get("count"):
        runs = ", ".join(span(r) for r in (clipping.get("runs") or [])[:LISTED])
        out.append(warning("AUDIO_CLIPPING", stage, f"the delivered audio clips in {clipping['count']} run(s): {runs}", count=clipping["count"]))
    silence = over_time.get("silence") or {}
    if music and silence.get("count"):
        runs = ", ".join(span(r) for r in (silence.get("runs") or [])[:LISTED])
        out.append(warning("AUDIO_SILENCE", stage, f"the delivered audio is silent in {silence['count']} run(s) despite the music bed: {runs}",
                           count=silence["count"]))
    timing = review.get("timing") or {}
    problems = [part for part in ("video", "audio") if isinstance(timing.get(part), dict) and timing[part].get("ok") is False]
    if problems:
        out.append(warning("TIMING_MISMATCH", stage, f"the delivered {' and '.join(problems)} length differs from the project",
                           **{p: timing[p] for p in problems}))
    cut = ((review.get("speech") or {}).get("cut_words") or {}).get("count") or 0
    if cut:
        out.append(warning("CUT_WORDS", stage, f"{cut} narrated word(s) are cut by clip edges", count=cut))
    meters = sound.get("meters") or {}
    if sound:
        lkfs = meters.get("integrated_lkfs")
        if lkfs is None:
            out.append(warning("LOUDNESS_UNMEASURED", stage, f"the delivered loudness could not be measured ({meters.get('loudness_status')})"))
        elif abs(lkfs - target_lkfs) > LOUDNESS_TOLERANCE_LU:
            out.append(warning("LOUDNESS_OFF_TARGET", stage, f"the delivered file measures {lkfs:.1f} LKFS against the target of "
                               f"{target_lkfs:g} LKFS (more than {LOUDNESS_TOLERANCE_LU:g} LU away)", integrated_lkfs=round(lkfs, 2),
                               target_lkfs=target_lkfs))
        peak = highest(meters.get("true_peak_dbtp")) if meters.get("true_peak_dbtp") is not None else highest(meters.get("sample_peak_dbfs"))
        if peak is not None and round(peak, 2) > peak_dbfs:
            detail = {"true_peak_dbtp": round(peak, 2), "peak_dbfs": peak_dbfs}
            if codec:
                detail["codec_trials"] = codec.get("trials")
            out.append(warning("PEAK_OVER_TARGET", stage, f"the delivered file's true peak is {peak:.2f} dBTP, above the manifest's "
                               f"peak_dbfs of {peak_dbfs:g}" + (f"; {codec_note(codec)}" if codec else ""), **detail))
    return out


def codec_warnings(codec, peak_dbfs, stage="mix"):
    """The mix found no limiter ceiling whose AAC trial encode stays under the target (reported when no review follows)."""
    if not codec or codec.get("passed"):
        return []
    return [warning("PEAK_OVER_TARGET", stage, f"no AAC trial encode of the mix stayed under peak_dbfs {peak_dbfs:g}; {codec_note(codec)}",
                    codec_trials=codec.get("trials"))]


def codec_note(codec):
    trials = codec.get("trials") or []
    tried = ", ".join(f"{t['ceiling_dbfs']:g} dBFS -> {t['true_peak_dbtp']:.2f} dBTP" for t in trials if t.get("true_peak_dbtp") is not None)
    return f"AAC trial encodes of the mix (limiter ceiling -> delivered true peak): {tried}" if tried else "no AAC trial was measured"


def highest(values):
    if isinstance(values, (int, float)):
        return float(values)
    found = [float(v) for v in (values or []) if isinstance(v, (int, float))]
    return max(found) if found else None


def seconds(value):
    if isinstance(value, dict):
        return value["num"] / value["den"]
    try:
        return float(value)
    except (TypeError, ValueError):
        return 0.0


def span(run):
    return f"{seconds(run.get('start')):.2f}-{seconds(run.get('end')):.2f} s"


def describe(difference):
    wanted, got = difference.get("expected") or "", difference.get("heard") or ""
    what = f"missing \"{wanted}\"" if wanted and not got else f"extra \"{got}\"" if got and not wanted else f"expected \"{wanted}\" heard \"{got}\""
    return f"{what} at {seconds(difference.get('start')):.2f} s"
