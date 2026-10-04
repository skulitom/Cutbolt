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
