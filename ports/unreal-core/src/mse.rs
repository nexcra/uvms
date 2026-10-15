//! Original application payload headers; an external WebSocket transport frames
//! these bytes. Caller provides already-muxed ISO-BMFF bytes and metadata XML.
use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tracks {
    Audio,
    Video,
    AudioVideo,
}

impl Tracks {
    pub fn flag(self, realtime: bool) -> u8 {
        let base = match self {
            Self::Audio => 1,
            Self::Video => 2,
            Self::AudioVideo => 3,
        };
        if realtime {
            base << 2
        } else {
            base
        }
    }
}

/// Copies native four u16 values into header offsets 2,4,6,8.
/// Fields are named by SDK consumption except the second word, unread by that JS.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct InitTimeshift {
    pub global_start: u16,
    pub native_second_word: u16,
    pub buffer_limit: u16,
    pub position: u16,
}

/// FUN_14009ba60 application payload; XML is UTF-16LE without a terminating NUL.
pub fn initialization(
    tracks: Tracks,
    realtime: bool,
    timeshift: InitTimeshift,
    metadata_xml: &str,
    initialization_segment: &[u8],
) -> Result<Vec<u8>, Error> {
    let xml: Vec<u16> = metadata_xml.encode_utf16().collect();
    let bytes = xml.len().checked_mul(2).ok_or(Error::TooLarge)?;
    // Native memcpy sign-extends a short. Avoid its malformed large-XML branch.
    if bytes > i16::MAX as usize {
        return Err(Error::TooLarge);
    }
    let total = 20usize
        .checked_add(bytes)
        .and_then(|n| n.checked_add(initialization_segment.len()))
        .ok_or(Error::TooLarge)?;
    // Native init outer WebSocket writer supports a 16-bit extended length only.
    if total > u16::MAX as usize {
        return Err(Error::TooLarge);
    }
    let mut out = vec![0u8; 20];
    out[0] = 1;
    out[1] = tracks.flag(realtime);
    for (offset, word) in [
        (2, timeshift.global_start),
        (4, timeshift.native_second_word),
        (6, timeshift.buffer_limit),
        (8, timeshift.position),
    ] {
        out[offset..offset + 2].copy_from_slice(&word.to_le_bytes());
    }
    out[18..20].copy_from_slice(&(bytes as u16).to_le_bytes());
    out.reserve(total - 20);
    for word in xml {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out.extend_from_slice(initialization_segment);
    Ok(out)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MediaTimeshift {
    pub in_progress: bool,
    /// None produces zero clock bytes; Some writes the original non-contiguous
    /// u24 layout. Clock derivation from the native state is outside this port.
    pub current_seconds: Option<u32>,
}

/// FUN_14009abf0 application payload. Timestamp is already derived by the caller;
/// native caller divides mux timestamp100ns by10000 and adds its session offset.
pub fn media(
    buffer_filling: u8,
    timestamp_ms: i32,
    timeshift: Option<MediaTimeshift>,
    segment: &[u8],
) -> Result<Vec<u8>, Error> {
    let total = segment.len().checked_add(10).ok_or(Error::TooLarge)?;
    if total > i32::MAX as usize {
        return Err(Error::TooLarge);
    }
    let header = media_header(buffer_filling, timestamp_ms, timeshift)?;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&header);
    out.extend_from_slice(segment);
    Ok(out)
}

/// Fixed application header, suitable for filling an existing mux prefix.
/// Has the same timestamp/timeshift layout as `media`, without heap allocation.
pub fn media_header(
    buffer_filling: u8,
    timestamp_ms: i32,
    timeshift: Option<MediaTimeshift>,
) -> Result<[u8; 10], Error> {
    let mut out = [0u8; 10];
    out[0] = 2;
    out[1] = buffer_filling;
    if let Some(t) = timeshift {
        out[2] = u8::from(!t.in_progress);
        if let Some(seconds) = t.current_seconds {
            if seconds == 0 || seconds > 0xffffff {
                return Err(Error::InvalidTimeshiftTime);
            }
            out[3] = (seconds >> 16) as u8;
            out[4..6].copy_from_slice(&(seconds as u16).to_le_bytes());
        }
    }
    out[6..10].copy_from_slice(&timestamp_ms.to_le_bytes());
    Ok(out)
}

/// FUN_14009ba60 error branch retains the stream mode flag in byte1.
pub fn error(tracks: Tracks, realtime: bool, message: &str) -> Result<Vec<u8>, Error> {
    let mut out = vec![0, tracks.flag(realtime)];
    for word in message.encode_utf16() {
        out.extend_from_slice(&word.to_le_bytes());
        if out.len() > u16::MAX as usize {
            return Err(Error::TooLarge);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod header_tests {
    use super::*;
    #[test]
    fn stack_header_preserves_native_signed_clock_and_noncontiguous_timeshift() {
        assert_eq!(
            media_header(7, -2, None).unwrap(),
            [2, 7, 0, 0, 0, 0, 254, 255, 255, 255]
        );
        let shift = Some(MediaTimeshift {
            in_progress: false,
            current_seconds: Some(0x123456),
        });
        assert_eq!(
            media_header(3, 0x11223344, shift).unwrap(),
            [2, 3, 1, 0x12, 0x56, 0x34, 0x44, 0x33, 0x22, 0x11]
        );
        assert_eq!(
            media(3, 0x11223344, shift, &[9, 8]).unwrap(),
            vec![2, 3, 1, 0x12, 0x56, 0x34, 0x44, 0x33, 0x22, 0x11, 9, 8]
        );
        for seconds in [0, 0x1000000] {
            assert!(media_header(
                0,
                0,
                Some(MediaTimeshift {
                    in_progress: true,
                    current_seconds: Some(seconds)
                })
            )
            .is_err());
        }
    }
}
