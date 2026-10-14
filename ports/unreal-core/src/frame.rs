//! UMediaServer.exe FUN_140025dc0 and FUN_140027e10, supported safe input subset.
use crate::Error;

const START: [u8; 4] = [0, 0, 0, 1];
pub const MAX_SOURCE_NALS: usize = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metadata {
    pub kind: u32,
    pub start_100ns: i64,
    pub end_100ns: i64,
    pub keyframe: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NalRange {
    /// Offset to the NAL header, excluding the four-byte start code.
    pub offset: u32,
    pub length: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ParameterSets<'a> {
    /// Raw NAL bytes including the NAL header, without length/start-code prefix.
    pub sps: &'a [u8],
    pub pps: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedFrame {
    pub metadata: Metadata,
    /// Annex B payload followed by native u32LE count and (offset,length) pairs.
    storage: Vec<u8>,
    payload_len: usize,
    nals: Vec<NalRange>,
}

impl NormalizedFrame {
    /// Transfer normalized payload, removing the trailer without copying bytes.
    pub fn into_payload(mut self) -> Vec<u8> {
        self.storage.truncate(self.payload_len);
        self.storage
    }
    pub fn payload(&self) -> &[u8] {
        &self.storage[..self.payload_len]
    }
    pub fn storage_with_index(&self) -> &[u8] {
        &self.storage
    }
    pub fn index_trailer(&self) -> &[u8] {
        &self.storage[self.payload_len..]
    }
    pub fn nals(&self) -> &[NalRange] {
        &self.nals
    }
}

/// Detects four-byte Annex B prefix exactly as native code; otherwise reads AVCC4.
/// Does not decode, re-encode, alter NAL bytes, derive timestamps, or infer keyframes.
/// Cache insertion mirrors native behavior: supplied SPS/PPS precede keyframes,
/// even when the frame already contains SPS/PPS. Cache discovery is not ported.
pub fn normalize(
    input: &[u8],
    metadata: Metadata,
    cache: Option<ParameterSets<'_>>,
) -> Result<NormalizedFrame, Error> {
    if input.is_empty() {
        return Err(Error::EmptyFrame);
    }
    let source = if input.starts_with(&START) {
        parse_annex_b(input)?
    } else {
        parse_avcc(input)?
    };
    let mut prefix = Vec::new();
    let mut nals = Vec::new();
    if metadata.keyframe {
        if let Some(cache) = cache {
            if cache.sps.first().map(|v| v & 31) != Some(7)
                || cache.pps.first().map(|v| v & 31) != Some(8)
                || cache.sps.len().saturating_add(cache.pps.len()) > 248
            {
                return Err(Error::InvalidParameterSets);
            }
            for nal in [cache.sps, cache.pps] {
                prefix.extend_from_slice(&START);
                nals.push(NalRange {
                    offset: prefix.len() as u32,
                    length: nal.len() as u32,
                });
                prefix.extend_from_slice(nal);
            }
        }
    }
    let payload_len = prefix
        .len()
        .checked_add(input.len())
        .ok_or(Error::TooLarge)?;
    let total = payload_len
        .checked_add(4 + (source.len() + nals.len()) * 8)
        .ok_or(Error::TooLarge)?;
    if total > i32::MAX as usize {
        return Err(Error::TooLarge);
    }
    let base = prefix.len();
    let mut storage = prefix;
    storage.reserve(total - storage.len());
    storage.extend_from_slice(input);
    for range in source {
        let offset = range.offset as usize + base;
        storage[offset - 4..offset].copy_from_slice(&START);
        nals.push(NalRange {
            offset: offset as u32,
            length: range.length,
        });
    }
    storage.extend_from_slice(&(nals.len() as u32).to_le_bytes());
    for range in &nals {
        storage.extend_from_slice(&range.offset.to_le_bytes());
        storage.extend_from_slice(&range.length.to_le_bytes());
    }
    Ok(NormalizedFrame {
        metadata,
        storage,
        payload_len,
        nals,
    })
}

fn add_range(ranges: &mut Vec<NalRange>, offset: usize, length: usize) -> Result<(), Error> {
    if ranges.len() == MAX_SOURCE_NALS {
        return Err(Error::TooManyNals);
    }
    if length == 0 {
        return Err(Error::InvalidNalLength);
    }
    ranges.push(NalRange {
        offset: u32::try_from(offset).map_err(|_| Error::TooLarge)?,
        length: u32::try_from(length).map_err(|_| Error::TooLarge)?,
    });
    Ok(())
}

fn parse_avcc(input: &[u8]) -> Result<Vec<NalRange>, Error> {
    let mut ranges = Vec::new();
    let mut cursor = 0;
    while cursor < input.len() {
        let header = input
            .get(cursor..cursor + 4)
            .ok_or(Error::TruncatedLength)?;
        let length = u32::from_be_bytes(header.try_into().unwrap()) as usize;
        // FUN_140027e10 stops here even if further AVCC NALs remain. We reject
        // that ambiguous subset instead of returning partially converted data.
        if length == 1 {
            return Err(Error::AmbiguousOneByteNal);
        }
        let offset = cursor + 4;
        let end = offset.checked_add(length).ok_or(Error::TooLarge)?;
        if end > input.len() {
            return Err(Error::InvalidNalLength);
        }
        add_range(&mut ranges, offset, length)?;
        cursor = end;
    }
    Ok(ranges)
}

fn parse_annex_b(input: &[u8]) -> Result<Vec<NalRange>, Error> {
    let mut ranges = Vec::new();
    let mut offset = 4;
    loop {
        let next = input[offset..]
            .windows(4)
            .position(|w| w == START)
            .map(|i| offset + i);
        let end = next.unwrap_or(input.len());
        add_range(&mut ranges, offset, end - offset)?;
        match next {
            Some(pos) => offset = pos + 4,
            None => break,
        }
    }
    Ok(ranges)
}
