//! Exact integration of authored linear speed ramps; no source-clock approximation.
use crate::{Result, error, time::Time};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sampling {
    Previous,
    Nearest,
    Linear,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioPitch {
    FollowSpeed,
    Mute,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub duration: Time,
    pub start_rate: Time,
    pub end_rate: Time,
    pub reverse: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Remap {
    pub segments: Vec<Segment>,
    pub video_sampling: Sampling,
    pub audio_pitch: AudioPitch,
}
#[derive(Serialize)]
pub(crate) struct Span {
    pub output_start: Time,
    pub output_end: Time,
    pub source_start: Time,
    pub source_end: Time,
    segment: Segment,
}
pub(crate) struct Compiled {
    pub spans: Vec<Span>,
}

impl Segment {
    fn distance(&self, t: Time) -> Result<Time> {
        // Integral of the endpoint-rate weighted line. Both terms are nonnegative,
        // including deceleration to zero; reverse is applied only to the position.
        let q = t
            .times(t)?
            .times(Time::new(self.duration.den, self.duration.num)?)?
            .times(Time::new(1, 2)?)?;
        self.start_rate
            .times(t.minus(q)?)?
            .plus(self.end_rate.times(q)?)
    }
}
impl Remap {
    pub(crate) fn compile(&self, start: Time, duration: Time) -> Result<Compiled> {
        start.validate()?;
        duration.validate()?;
        if self.segments.is_empty() || self.segments.len() > 64 {
            return Err(error(
                "INVALID_REMAP",
                "Remapping requires 1..64 speed segments",
            ));
        }
        let mut clock = Time::ZERO;
        let mut source = start;
        let mut spans = Vec::new();
        for segment in &self.segments {
            if segment.duration.units(Time::new(48000, 1)?)? == 0
                || segment.start_rate.compare(Time::new(16, 1)?)? == Ordering::Greater
                || segment.end_rate.compare(Time::new(16, 1)?)? == Ordering::Greater
            {
                return Err(error(
                    "INVALID_REMAP",
                    "Segment duration must be positive on the 48 kHz clock; rates must be 0..16",
                ));
            }
            if matches!(self.audio_pitch, AudioPitch::FollowSpeed)
                && (segment.reverse || (segment.start_rate.num == 0 && segment.end_rate.num == 0))
            {
                return Err(error(
                    "INVALID_REMAP",
                    "Reverse and frozen segments require the explicit mute pitch policy",
                ));
            }
            let output_end = clock.plus(segment.duration)?;
            if output_end.compare(duration)? == Ordering::Greater {
                return Err(error(
                    "INVALID_REMAP",
                    "Speed segments exceed the output duration",
                ));
            }
            let distance = segment.distance(segment.duration)?;
            let source_end = if segment.reverse {
                source.minus(distance)?
            } else {
                source.plus(distance)?
            };
            spans.push(Span {
                output_start: clock,
                output_end,
                source_start: source,
                source_end,
                segment: segment.clone(),
            });
            clock = output_end;
            source = source_end;
        }
        if clock.compare(duration)? != Ordering::Equal {
            return Err(error(
                "INVALID_REMAP",
                "Speed segments must exactly cover the output duration",
            ));
        }
        Ok(Compiled { spans })
    }
}
impl Compiled {
    pub fn source_time(&self, t: Time) -> Result<Time> {
        t.validate()?;
        let index = self.spans.partition_point(|span| {
            span.output_end.compare(t).expect("validated clock") != Ordering::Greater
        });
        let span = &self.spans[index.min(self.spans.len() - 1)];
        if t.compare(span.output_end)? == Ordering::Greater {
            return Err(error("INVALID_RANGE", "Output clock exceeds speed map"));
        }
        let distance = span.segment.distance(t.minus(span.output_start)?)?;
        if span.segment.reverse {
            span.source_start.minus(distance)
        } else {
            span.source_start.plus(distance)
        }
    }
}
