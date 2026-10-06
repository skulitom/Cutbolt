//! Exact native frame clocks and declared container timestamp quantization.
use super::*;

pub(crate) fn rate(value: Time) -> Result<Time> {
    let value = Time::new(value.num, value.den)?;
    if ![
        (24, 1),
        (25, 1),
        (30, 1),
        (50, 1),
        (60, 1),
        (24000, 1001),
        (30000, 1001),
        (60000, 1001),
    ]
    .iter()
    .any(|&(num, den)| value == Time { num, den })
    {
        return Err(error(
            "UNSUPPORTED_TIMELINE",
            "Native sequential rendering supports 24, 25, 30, 50, 60, 24000/1001, 30000/1001 or 60000/1001 fps",
        ));
    }
    Ok(value)
}

pub(crate) fn rate_hint(video: &Value, metadata: &Value, rate: Time) -> bool {
    if video["r_frame_rate"] == format!("{}/{}", rate.num, rate.den) {
        return true;
    }
    if rate == (Time { num: 25, den: 1 })
        || !metadata["format"]["format_name"]
            .as_str()
            .unwrap_or("")
            .split(',')
            .any(|s| s == "matroska")
    {
        return false;
    }
    let Some((n, d)) = video["r_frame_rate"]
        .as_str()
        .and_then(|s| s.split_once('/'))
    else {
        return false;
    };
    let (Ok(n), Ok(d)) = (n.parse::<u64>(), d.parse::<u64>()) else {
        return false;
    };
    if n == 0 || d == 0 {
        return false;
    }
    // Matroska's nominal DefaultDuration uses integer nanoseconds. Treat this as
    // a precision-limited hint; every decoded timestamp must still fit the exact clock.
    let a = d as u128 * rate.num as u128;
    let b = rate.den as u128 * n as u128;
    a.abs_diff(b) * 1_000_000_000 <= n as u128 * rate.num as u128
}

pub(crate) fn timestamp(frame: &Value, index: usize, rate: Time, video: &Value) -> Result<()> {
    let observed = micros(&frame["best_effort_timestamp_time"])?;
    if rate == (Time { num: 25, den: 1 }) {
        // Preserve the existing exact 40 ms acceptance for the original profile.
        if observed == index as i64 * 40_000 {
            return Ok(());
        }
    } else {
        let (n, d) = video["time_base"]
            .as_str()
            .and_then(|v| v.split_once('/'))
            .ok_or_else(|| error("UNSUPPORTED_MEDIA", "Missing container time base"))?;
        let n: u64 = n
            .parse()
            .map_err(|_| error("UNSUPPORTED_MEDIA", "Invalid container time base"))?;
        let d: u64 = d
            .parse()
            .map_err(|_| error("UNSUPPORTED_MEDIA", "Invalid container time base"))?;
        if n == 0 || d == 0 || n as u128 * 1000 > d as u128 || d as u128 > n as u128 * 1_000_000_000
        {
            return Err(error(
                "UNSUPPORTED_MEDIA",
                "Native frame timestamps require a container time base of 1 ns through 1 ms",
            ));
        }
        let numerator = index as u128 * rate.den as u128 * d as u128;
        let denominator = rate.num as u128 * n as u128;
        let ticks = (numerator + denominator / 2) / denominator;
        let expected = (ticks * n as u128 * 1_000_000 + d as u128 / 2) / d as u128;
        // Probe's textual seconds carry six decimal places; the integer clock is exact.
        if (observed as i128 - expected as i128).abs() <= 1 {
            return Ok(());
        }
    }
    Err(error(
        "UNSUPPORTED_MEDIA",
        "Video timestamps do not match the zero-origin native frame clock at the declared container precision; convert VFR sources explicitly",
    ))
}

/// `thirds` thirds of a frame period at `rate`, floored to the microsecond, in FFmpeg's seconds.
fn thirds(rate: Time, thirds: u64) -> String {
    let micros = u128::from(thirds) * u128::from(rate.den) * 1_000_000 / (3 * u128::from(rate.num));
    let (whole, fraction) = (micros / 1_000_000, micros % 1_000_000);
    if fraction == 0 {
        whole.to_string()
    } else {
        format!("{whole}.{}", format!("{fraction:06}").trim_end_matches('0'))
    }
}

/// Input options that start reading a source at frame `first` (none for the first frame).
///
/// Inspection tied every frame of a source to its index (`timestamp`): frame `n` is timed `n`
/// periods, at the container's precision. Reading starts a third of a period before `first`.
/// FFmpeg moves the demuxer to the last keyframe at or before that time (every FFV1 frame is one),
/// counts time from it and drops earlier frames; `seek_trim` then keeps the wanted frames by time.
/// Each boundary lies at least a third of a period (5.5 ms at 60 fps) from every frame time. The
/// container clock, the offset and the trim each round by at most half a millisecond on the 1 ms
/// and finer clocks inspection accepts, and a coarser clock that passes inspection divides the
/// period, so no frame can cross a boundary. Frames are thus chosen by their own timestamps: a seek
/// that landed late could only lose frames, never show others, and every reader of a trimmed
/// picture requires its exact frame count.
pub(crate) fn seek_input(rate: Time, first: u64) -> Vec<String> {
    if first == 0 {
        return Vec::new();
    }
    vec!["-ss".into(), thirds(rate, 3 * first - 1)]
}

/// The trim keeping source frames `[first, end)` of an input started at frame `seek` by
/// `seek_input` (`seek <= first`). Unsought inputs keep counting frames from the start.
pub(crate) fn seek_trim(rate: Time, seek: u64, first: u64, end: u64) -> String {
    if seek == 0 {
        return format!("trim=start_frame={first}:end_frame={end}");
    }
    format!(
        "trim=start={}:end={}",
        thirds(rate, 3 * (first - seek)),
        thirds(rate, 3 * (end - seek))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeks_start_a_third_of_a_frame_early_and_trim_on_frame_times() {
        let pal = Time { num: 25, den: 1 };
        assert!(seek_input(pal, 0).is_empty());
        assert_eq!(seek_input(pal, 2790), ["-ss", "111.586666"]);
        assert_eq!(seek_trim(pal, 0, 5, 17), "trim=start_frame=5:end_frame=17");
        assert_eq!(seek_trim(pal, 2790, 2790, 2802), "trim=start=0:end=0.48");
        assert_eq!(seek_trim(pal, 100, 130, 131), "trim=start=1.2:end=1.24");
        let ntsc = Time {
            num: 30000,
            den: 1001,
        };
        // 10 periods are 333,666.66 µs; a third before frame 10 is 322,544.44 µs.
        assert_eq!(seek_input(ntsc, 10), ["-ss", "0.322544"]);
        assert_eq!(seek_trim(ntsc, 10, 10, 20), "trim=start=0:end=0.333666");
    }
}
