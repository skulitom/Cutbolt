//! Bounded JSON adapter for the independent Python working-color oracle; no media or jobs.
use cutbolt::{
    color::{
        Transfer,
        working::{Dither, Pixel},
    },
    composite::{AlphaMode, BlendMode},
};
use serde::Deserialize;
use serde_json::json;
use std::io::{self, Read};

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Operation {
    Decode {
        rgba: [u8; 4],
        transfer: Transfer,
        alpha: AlphaMode,
    },
    Store {
        rgba: [f64; 4],
    },
    Over {
        source: [u16; 4],
        backdrop: [u16; 4],
        opacity: u16,
        mask: u32,
        mode: BlendMode,
    },
    Mix {
        first: [u16; 4],
        second: [u16; 4],
        weight: u16,
    },
    Deliver {
        rgba: [u16; 4],
        transfer: Transfer,
        position: [u32; 2],
        ordered: bool,
    },
}

fn main() -> cutbolt::Result<()> {
    let mut input = Vec::new();
    io::stdin()
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut input)?;
    if input.len() > 16 * 1024 * 1024 {
        return Err(cutbolt::error(
            "LIMIT_EXCEEDED",
            "Fixture input exceeds 16 MiB",
        ));
    }
    let operations: Vec<Operation> = serde_json::from_slice(&input)?;
    if operations.len() > 65536 {
        return Err(cutbolt::error(
            "LIMIT_EXCEEDED",
            "Fixture exceeds 65536 operations",
        ));
    }
    let mut result = Vec::with_capacity(operations.len());
    for op in operations {
        result.push(match op {
            Operation::Decode {
                rgba,
                transfer,
                alpha,
            } => json!(Pixel::from_encoded8(rgba, transfer, alpha)?.0),
            Operation::Store { rgba } => json!(Pixel::from_linear(rgba)?.0),
            Operation::Over {
                source,
                backdrop,
                opacity,
                mask,
                mode,
            } => json!(Pixel(source).over(Pixel(backdrop), opacity, mask, mode)?.0),
            Operation::Mix {
                first,
                second,
                weight,
            } => json!(Pixel(first).mix(Pixel(second), weight).0),
            Operation::Deliver {
                rgba,
                transfer,
                position,
                ordered,
            } => json!(Pixel(rgba).encoded8(
                transfer,
                position,
                if ordered {
                    Dither::Ordered8x8
                } else {
                    Dither::Nearest
                }
            )),
        });
    }
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}
