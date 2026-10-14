use unreal_mp4::{AudioConfig, Muxer};
fn mux() -> Muxer {
    Muxer::new_av(
        &[0x67, 0x42, 0xe0, 0x1e],
        &[0x68, 0xce],
        640,
        360,
        Some(AudioConfig {
            sample_rate: 48000,
            channels: 2,
            decoder_config: vec![0x11, 0x90],
        }),
    )
    .unwrap()
}
fn u32at(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(data[at..at + 4].try_into().unwrap())
}
#[test]
fn aac_track_and_independent_clock_preserve_sample_bytes() {
    let mut m = mux();
    assert_eq!(m.codec(), "avc1.42E01E,mp4a.40.2");
    for expected in [b"mp4a", b"esds", b"soun"] {
        assert!(m.init_segment().windows(4).any(|w| w == expected));
    }
    assert_eq!(
        m.init_segment()
            .windows(4)
            .filter(|w| *w == b"trak")
            .count(),
        2
    );
    assert_eq!(
        m.init_segment()
            .windows(4)
            .filter(|w| *w == b"trex")
            .count(),
        2
    );
    assert!(m.audio_fragment(&[1, 2], 0, 1024).is_err());
    m.fragment(&[0, 0, 0, 2, 0x65, 0xb0], 0, 6000, true)
        .unwrap();
    let data = [0x21, 0x10, 0x56, 0xe5];
    let sample = m
        .audio_fragment_with_prefix(&data, 0x100000005, 1024, 10)
        .unwrap();
    assert_eq!(&sample[..10], &[0; 10]);
    let mp4 = &sample[10..];
    assert_eq!(u32at(mp4, 20), 2); // global mfhd sequence after video
    assert_eq!(u32at(mp4, 44), 2); // audio track id
    assert_eq!(
        u64::from_be_bytes(mp4[60..68].try_into().unwrap()),
        0x100000005
    );
    assert_eq!(u32at(mp4, 84), 108);
    assert_eq!(u32at(mp4, 88), 1024);
    assert_eq!(&mp4[108..], &data);
    assert!(m.audio_fragment(&data, 0x100000005, 1024).is_err());
    let next = m
        .fragment(&[0, 0, 0, 2, 0x61, 0xc0], 6000, 6000, false)
        .unwrap();
    assert_eq!(u32at(&next, 20), 3);
}
#[test]
fn aac_configuration_and_malformed_frames_are_rejected() {
    for (rate, channels, asc) in [
        (44100, 2, vec![0x11, 0x90]),
        (48000, 1, vec![0x11, 0x90]),
        (48000, 2, vec![0x29, 0x90]),
        (48000, 2, vec![0x11, 0x94]),
    ] {
        assert!(Muxer::new_av(
            &[0x67, 0x42, 0xe0, 0x1e],
            &[0x68, 0xce],
            640,
            360,
            Some(AudioConfig {
                sample_rate: rate,
                channels,
                decoder_config: asc
            })
        )
        .is_err());
    }
    let mut m = mux();
    m.fragment(&[0, 0, 0, 2, 0x65, 0xb0], 0, 6000, true)
        .unwrap();
    assert!(m.audio_fragment(&[], 0, 1024).is_err());
    assert!(m.audio_fragment(&[1], 0, 960).is_err());
    assert!(m.audio_fragment(&[1], u64::MAX, 1024).is_err());
    assert!(m.audio_fragment_with_prefix(&[1], 0, 1024, 65).is_err());
    assert_eq!(u32at(&m.audio_fragment(&[1], 0, 1024).unwrap(), 20), 2);
}
