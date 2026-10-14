use unreal_core_port::{
    frame::{self, Metadata, ParameterSets},
    mse::{self, InitTimeshift, MediaTimeshift, Tracks},
    Error,
};

fn metadata(keyframe: bool) -> Metadata {
    Metadata {
        kind: 2,
        start_100ns: 7_333_333,
        end_100ns: 8_000_000,
        keyframe,
    }
}

#[test]
fn native_index_layout_preserves_payload_and_metadata() {
    let source = [0, 0, 0, 3, 0x65, 0xab, 0xcd, 0, 0, 0, 2, 0x61, 0xee];
    let result = frame::normalize(&source, metadata(true), None).unwrap();
    assert_eq!(
        result.payload(),
        [0, 0, 0, 1, 0x65, 0xab, 0xcd, 0, 0, 0, 1, 0x61, 0xee]
    );
    assert_eq!(
        result.index_trailer(),
        [2, 0, 0, 0, 4, 0, 0, 0, 3, 0, 0, 0, 11, 0, 0, 0, 2, 0, 0, 0]
    );
    assert_eq!(result.metadata, metadata(true));
}

#[test]
fn four_byte_annex_b_is_preserved() {
    let source = [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xee];
    let result = frame::normalize(&source, metadata(false), None).unwrap();
    assert_eq!(result.payload(), source);
    assert_eq!(
        result
            .nals()
            .iter()
            .map(|n| (n.offset, n.length))
            .collect::<Vec<_>>(),
        [(4, 2), (10, 2)]
    );
}

#[test]
fn cache_precedes_keyframe_only_and_does_not_deduplicate() {
    let cache = ParameterSets {
        sps: &[0x67, 0x42],
        pps: &[0x68, 0xee],
    };
    let source = [0, 0, 0, 2, 0x65, 0x80];
    let key = frame::normalize(&source, metadata(true), Some(cache)).unwrap();
    assert_eq!(
        key.payload(),
        [0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xee, 0, 0, 0, 1, 0x65, 0x80]
    );
    assert_eq!(
        key.nals().iter().map(|n| n.offset).collect::<Vec<_>>(),
        [4, 10, 16]
    );
    let nonkey = frame::normalize(&source, metadata(false), Some(cache)).unwrap();
    assert_eq!(nonkey.nals().len(), 1);
    assert_eq!(nonkey.metadata, metadata(false));
}

#[test]
fn malformed_and_native_ambiguous_inputs_are_rejected() {
    for (input, expected) in [
        (vec![], Error::EmptyFrame),
        (vec![0, 0, 0], Error::TruncatedLength),
        (vec![0, 0, 0, 4, 0x65], Error::InvalidNalLength),
        (vec![0, 0, 0, 0], Error::InvalidNalLength),
        (vec![0, 0, 0, 1], Error::InvalidNalLength),
        (
            vec![0, 0, 0, 2, 0x61, 0x80, 0, 0, 0, 1, 0x09],
            Error::AmbiguousOneByteNal,
        ),
    ] {
        assert_eq!(
            frame::normalize(&input, metadata(false), None).unwrap_err(),
            expected
        );
    }
    // Same ambiguous prefix as original selects Annex B, not AVCC1.
    assert_eq!(
        frame::normalize(&[0, 0, 0, 1, 0x09], metadata(false), None)
            .unwrap()
            .nals()[0]
            .length,
        1
    );
}

#[test]
fn sixty_source_nals_allowed_but_sixty_one_rejected() {
    let mut source = [0, 0, 0, 2, 0x61, 0x80].repeat(60);
    assert_eq!(
        frame::normalize(&source, metadata(false), None)
            .unwrap()
            .nals()
            .len(),
        60
    );
    source.extend_from_slice(&[0, 0, 0, 2, 0x61, 0x80]);
    assert_eq!(
        frame::normalize(&source, metadata(false), None).unwrap_err(),
        Error::TooManyNals
    );
}

#[test]
fn init_utf16_length_and_all_native_mode_flags() {
    let modes = [
        (Tracks::Audio, 1, 4),
        (Tracks::Video, 2, 8),
        (Tracks::AudioVideo, 3, 12),
    ];
    for (tracks, buffered, realtime) in modes {
        assert_eq!(tracks.flag(false), buffered);
        assert_eq!(tracks.flag(true), realtime);
    }
    let packet = mse::initialization(
        Tracks::Video,
        true,
        InitTimeshift::default(),
        "가😀",
        &[0xde, 0xad],
    )
    .unwrap();
    assert_eq!(
        packet,
        [
            1, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 0, 0, 0xac, 0x3d, 0xd8, 0,
            0xde, 0xde, 0xad
        ]
    );
}

#[test]
fn timeshift_headers_follow_original_noncontiguous_layout() {
    let init = mse::initialization(
        Tracks::Audio,
        false,
        InitTimeshift {
            global_start: 0x1234,
            native_second_word: 0x5678,
            buffer_limit: 0x9abc,
            position: 0xdef0,
        },
        "",
        &[],
    )
    .unwrap();
    assert_eq!(
        &init[2..10],
        &[0x34, 0x12, 0x78, 0x56, 0xbc, 0x9a, 0xf0, 0xde]
    );
    let packet = mse::media(
        1,
        -100,
        Some(MediaTimeshift {
            in_progress: false,
            current_seconds: Some(0x123456),
        }),
        &[0xaa],
    )
    .unwrap();
    assert_eq!(
        packet,
        [2, 1, 1, 0x12, 0x56, 0x34, 0x9c, 0xff, 0xff, 0xff, 0xaa]
    );
    let live = mse::media(0, 100, None, &[0xbb]).unwrap();
    assert_eq!(live, [2, 0, 0, 0, 0, 0, 100, 0, 0, 0, 0xbb]);
}

#[test]
fn payload_bounds_reject_original_signed_length_hazards() {
    assert_eq!(
        mse::initialization(
            Tracks::Video,
            false,
            InitTimeshift::default(),
            &"a".repeat(16384),
            &[]
        ),
        Err(Error::TooLarge)
    );
    assert_eq!(
        mse::initialization(
            Tracks::Video,
            false,
            InitTimeshift::default(),
            "",
            &vec![0; 65516]
        ),
        Err(Error::TooLarge)
    );
    for seconds in [0, 0x1000000] {
        assert_eq!(
            mse::media(
                0,
                0,
                Some(MediaTimeshift {
                    in_progress: true,
                    current_seconds: Some(seconds)
                }),
                &[]
            ),
            Err(Error::InvalidTimeshiftTime)
        );
    }
    assert_eq!(
        mse::error(Tracks::Video, true, "오류").unwrap(),
        [0, 8, 0x24, 0xc6, 0x58, 0xb9]
    );
}
