//! Write standard display fields into reserved space in our own newly encoded Matroska file.
//! Element offsets and lengths stay fixed; encoded blocks and cue/seek offsets are untouched.
use crate::{
    Result,
    hdr_color::{Encoding, invalid},
};
use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

pub(crate) fn reservation() -> String {
    format!("cutbolt-display-reserve-{}", "x".repeat(512))
}
#[derive(Clone, Copy)]
struct Element {
    id: u64,
    start: usize,
    body: usize,
    end: usize,
}
fn vint(data: &[u8], at: usize, id: bool) -> Result<(u64, usize)> {
    let first = *data
        .get(at)
        .ok_or_else(|| invalid("Truncated generated container"))?;
    let n = first.leading_zeros() as usize + 1;
    if n > if id { 4 } else { 8 } || at.checked_add(n).is_none_or(|e| e > data.len()) {
        return Err(invalid("Invalid generated EBML integer"));
    }
    let mut value = if id {
        first as u64
    } else {
        (first & (0xffu16 >> n) as u8) as u64
    };
    for byte in &data[at + 1..at + n] {
        value = (value << 8) | *byte as u64;
    }
    Ok((value, n))
}
fn header(data: &[u8], at: usize) -> Result<Element> {
    let (id, n) = vint(data, at, true)?;
    let (size, k) = vint(data, at + n, false)?;
    let body = at + n + k;
    let end = if size == (1u64 << (7 * k)) - 1 && id == 0x18538067 {
        usize::MAX
    } else {
        body.checked_add(usize::try_from(size).map_err(|_| invalid("Container length overflow"))?)
            .ok_or_else(|| invalid("Container length overflow"))?
    };
    Ok(Element {
        id,
        start: at,
        body,
        end,
    })
}
fn children(data: &[u8]) -> Result<Vec<Element>> {
    let mut result = Vec::new();
    let mut at = 0;
    while at < data.len() {
        let e = header(data, at)?;
        if e.end > data.len() || e.id == 0xbf || result.len() > 4096 {
            return Err(invalid("Unexpected container CRC, bounds or element count"));
        }
        result.push(e);
        at = e.end;
    }
    Ok(result)
}
fn element(id: u64, body: &[u8]) -> Vec<u8> {
    let id_bytes = id.to_be_bytes();
    let first = id_bytes.iter().position(|b| *b != 0).expect("nonzero ID");
    let mut out = id_bytes[first..].to_vec();
    let size = body.len() as u64;
    let n = (1..=8)
        .find(|n| size < (1u64 << (7 * n)) - 1)
        .expect("bounded payload");
    let encoded = (size | (1u64 << (7 * n))).to_be_bytes();
    out.extend_from_slice(&encoded[8 - n..]);
    out.extend_from_slice(body);
    out
}
fn display(encoding: Encoding) -> Vec<u8> {
    let mut body = Vec::new();
    let mut values: Vec<f64> = encoding.primaries.xy().into_iter().flatten().collect();
    values.extend([
        0.3127,
        0.3290,
        encoding.display.peak_nits as f64,
        encoding.display.black_millinits as f64 / 1000.0,
    ]);
    for (i, v) in values.iter().enumerate() {
        body.extend(element(0x55d1 + i as u64, &v.to_be_bytes()));
    }
    element(0x55d0, &body)
}
fn video(data: &[u8], encoding: Encoding) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut found = false;
    for e in children(data)? {
        if e.id == 0x55b0 {
            if found {
                return Err(invalid("Duplicate generated color metadata"));
            }
            found = true;
            let mut colour = Vec::new();
            let old = &data[e.body..e.end];
            for child in children(old)? {
                if matches!(child.id, 0x55d0 | 0x55bc | 0x55bd) {
                    return Err(invalid("Unexpected existing HDR metadata"));
                }
                colour.extend_from_slice(&old[child.start..child.end]);
            }
            colour.extend(display(encoding));
            body.extend(element(0x55b0, &colour));
        } else {
            body.extend_from_slice(&data[e.start..e.end]);
        }
    }
    if !found {
        return Err(invalid("Generated color metadata missing"));
    }
    Ok(element(0xe0, &body))
}
fn padded_entry(data: &[u8], encoding: Encoding) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut names = 0;
    let mut videos = 0;
    for e in children(data)? {
        match e.id {
            0x536e => {
                if &data[e.body..e.end] != reservation().as_bytes() {
                    return Err(invalid("Unexpected generated track name"));
                }
                names += 1;
            }
            0xe0 => {
                body.extend(video(&data[e.body..e.end], encoding)?);
                videos += 1;
            }
            _ => body.extend_from_slice(&data[e.start..e.end]),
        }
    }
    if names != 1 || videos != 1 || body.len() + 2 > data.len() {
        return Err(invalid("Display metadata reservation missing or too short"));
    }
    let space = data.len() - body.len();
    for n in 1..=8 {
        if space > n && ((space - 1 - n) as u64) < (1u64 << (7 * n)) - 1 {
            body.push(0xec);
            let size = ((space - 1 - n) as u64 | (1u64 << (7 * n))).to_be_bytes();
            body.extend_from_slice(&size[8 - n..]);
            body.resize(data.len(), 0);
            return Ok(body);
        }
    }
    Err(invalid("Invalid padding reservation"))
}
pub(crate) fn write(path: &Path, encoding: Encoding) -> Result<()> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut data = Vec::new();
    (&mut file).take(1024 * 1024).read_to_end(&mut data)?;
    let ebml = header(&data, 0)?;
    if ebml.id != 0x1a45dfa3 || ebml.end > data.len() {
        return Err(invalid("Generated EBML header missing"));
    }
    let fields = children(&data[ebml.body..ebml.end])?;
    if !fields
        .iter()
        .any(|e| e.id == 0x4287 && data.get(ebml.body + e.body) == Some(&4))
    {
        return Err(invalid(
            "Generated container must declare Matroska version 4",
        ));
    }
    let segment = header(&data, ebml.end)?;
    if segment.id != 0x18538067 {
        return Err(invalid("Generated Matroska segment missing"));
    }
    let mut at = segment.body;
    while at < data.len().min(segment.end) {
        let e = header(&data, at)?;
        if e.end > data.len() {
            break;
        }
        if e.id == 0x1654ae6b {
            let track_data = &data[e.body..e.end];
            let mut patched = 0;
            for track in children(track_data)? {
                if track.id == 0xae {
                    let body = &track_data[track.body..track.end];
                    if children(body)?.iter().any(|c| c.id == 0xe0) {
                        let replacement = padded_entry(body, encoding)?;
                        file.seek(SeekFrom::Start((e.body + track.body) as u64))?;
                        file.write_all(&replacement)?;
                        patched += 1;
                    }
                }
            }
            if patched != 1 {
                return Err(invalid("Exactly one reserved video track required"));
            }
            file.sync_all()?;
            return Ok(());
        }
        at = e.end;
    }
    Err(invalid("Generated track header exceeds metadata bounds"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn metadata_lengths_truncation_and_reserved_payload() {
        for length in [0, 1, 126, 127, 128, 16382, 16383] {
            let bytes = element(0x55d0, &vec![0; length]);
            let e = header(&bytes, 0).unwrap();
            assert_eq!(e.end, bytes.len());
            assert_eq!(e.end - e.body, length);
        }
        for raw in [
            &[][..],
            &[0][..],
            &[0x55][..],
            &[0x55, 0xd0, 0xff][..],
            &[0xbf, 0x84, 0, 0, 0, 0][..],
        ] {
            assert!(header(raw, 0).is_err() || children(raw).is_err());
        }
        let encoding = Encoding {
            transfer: crate::hdr_color::Transfer::Pq,
            primaries: crate::hdr_color::Primaries::Bt2020,
            display: crate::hdr_color::Display {
                peak_nits: 1000,
                black_millinits: 5,
            },
        };
        let mut entry = element(0x536e, reservation().as_bytes());
        entry.extend(element(0xe0, &element(0x55b0, &element(0x55b1, &[0]))));
        let patched = padded_entry(&entry, encoding).unwrap();
        assert_eq!(patched.len(), entry.len());
        assert!(children(&patched).unwrap().iter().all(|e| e.id != 0x536e));
        assert!(padded_entry(&patched, encoding).is_err());
    }
}
