use unreal_mp4::Muxer;
const SPS: &[u8] = &[0x67, 0x42, 0xe0, 0x1e];
const PPS: &[u8] = &[0x68, 0xce, 0x3c, 0x80];
fn u32at(bytes: &[u8], p: usize) -> u32 {
    u32::from_be_bytes(bytes[p..p + 4].try_into().unwrap())
}
fn children(bytes: &[u8]) -> Vec<(&[u8], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        assert!(bytes.len() - at >= 8);
        let size = u32at(bytes, at) as usize;
        assert!(size >= 8 && size <= bytes.len() - at);
        out.push((&bytes[at + 4..at + 8], &bytes[at + 8..at + size]));
        at += size;
    }
    assert_eq!(at, bytes.len());
    out
}
fn child<'a>(bytes: &'a [u8], kind: &[u8]) -> &'a [u8] {
    children(bytes)
        .into_iter()
        .find(|(k, _)| *k == kind)
        .unwrap()
        .1
}
fn sample(nal: &[u8]) -> Vec<u8> {
    [(nal.len() as u32).to_be_bytes().as_slice(), nal].concat()
}

#[test]
fn init_has_empty_tables_avcc_and_fragment_defaults() {
    let mux = Muxer::new(SPS, PPS, 640, 360).unwrap();
    assert_eq!(mux.codec(), "avc1.42E01E");
    let top = children(mux.init_segment());
    assert_eq!(
        top.iter().map(|(k, _)| *k).collect::<Vec<_>>(),
        [b"ftyp", b"moov"]
    );
    assert_eq!(&top[0].1[..8], b"isom\0\0\x02\0");
    let moov = top[1].1;
    assert_eq!(u32at(child(moov, b"mvhd"), 12), 1000);
    let trak = child(moov, b"trak");
    let tkhd = child(trak, b"tkhd");
    assert_eq!(u32at(tkhd, tkhd.len() - 8), 640 << 16);
    assert_eq!(u32at(tkhd, tkhd.len() - 4), 360 << 16);
    let mdia = child(trak, b"mdia");
    assert_eq!(u32at(child(mdia, b"mdhd"), 12), 90000);
    let stbl = child(child(mdia, b"minf"), b"stbl");
    for kind in [b"stts", b"stsc", b"stco"] {
        assert_eq!(child(stbl, kind), [0; 8]);
    }
    assert_eq!(child(stbl, b"stsz"), [0; 12]);
    let stsd = child(stbl, b"stsd");
    assert_eq!(u32at(stsd, 4), 1);
    let avc1 = child(&stsd[8..], b"avc1");
    let config = child(&avc1[78..], b"avcC");
    assert_eq!(&config[..6], &[1, 0x42, 0xe0, 0x1e, 0xff, 0xe1]);
    assert_eq!(&config[8..12], SPS);
    let trex = child(child(moov, b"mvex"), b"trex");
    assert_eq!(u32at(trex, 4), 1);
    assert_eq!(u32at(trex, 8), 1);
}

#[test]
fn fragments_address_mdat_and_preserve_bytes() {
    let mut mux = Muxer::new(SPS, PPS, 640, 360).unwrap();
    for (index, timestamp, duration, key, nal) in [
        (1, 0, 6000, true, vec![0x65, 0xb0, 0xab]),
        (2, 6000, 3000, false, vec![0x61, 0xc0, 0xcd]),
        (3, u32::MAX as u64 + 3, 9000, false, vec![0x61, 0xc0, 0xee]),
    ] {
        let input = sample(&nal);
        let fragment = mux.fragment(&input, timestamp, duration, key).unwrap();
        let boxes = children(&fragment);
        assert_eq!(boxes.len(), 2);
        assert_eq!(boxes[0].0, b"moof");
        assert_eq!(boxes[1].0, b"mdat");
        assert_eq!(boxes[1].1, input);
        let moof = boxes[0].1;
        assert_eq!(u32at(child(moof, b"mfhd"), 4), index);
        let traf = child(moof, b"traf");
        let tfhd = child(traf, b"tfhd");
        assert_eq!(&tfhd[..4], &[0, 2, 0, 0]);
        let tfdt = child(traf, b"tfdt");
        assert_eq!(tfdt[0], 1);
        assert_eq!(
            u64::from_be_bytes(tfdt[4..12].try_into().unwrap()),
            timestamp
        );
        let trun = child(traf, b"trun");
        assert_eq!(&trun[..4], &[0, 0, 7, 1]);
        assert_eq!(u32at(trun, 4), 1);
        assert_eq!(u32at(trun, 8) as usize, boxes[0].1.len() + 16);
        assert_eq!(u32at(trun, 12), duration);
        assert_eq!(u32at(trun, 16) as usize, input.len());
        assert_eq!(u32at(trun, 20), if key { 0x02000000 } else { 0x01010000 });
    }
}

#[test]
fn invalid_samples_do_not_advance_sequence() {
    let mut mux = Muxer::new(SPS, PPS, 640, 360).unwrap();
    assert!(mux
        .fragment(&sample(&[0x61, 0xc0]), 0, 6000, false)
        .is_err());
    assert!(mux.fragment(&sample(&[0x65, 0xa0]), 0, 6000, true).is_err()); // B slice
    assert!(mux.fragment(&[0, 0, 0, 100, 0x65], 0, 6000, true).is_err());
    assert!(mux.fragment(&sample(&[0x65, 0xb0]), 0, 0, true).is_err());
    let first = mux.fragment(&sample(&[0x65, 0xb0]), 0, 6000, true).unwrap();
    assert_eq!(u32at(child(child(&first, b"moof"), b"mfhd"), 4), 1);
    assert!(mux
        .fragment(&sample(&[0x61, 0xc0]), 0, 6000, false)
        .is_err());
    let changed = [sample(&[0x67, 0x42, 0x80, 0x1e]), sample(&[0x65, 0xb0])].concat();
    assert!(mux.fragment(&changed, 6000, 6000, true).is_err());
}

#[test]
fn init_validates_dimensions_and_parameter_set_types() {
    assert!(Muxer::new(SPS, PPS, 0, 360).is_err());
    assert!(Muxer::new(SPS, PPS, 65536, 360).is_err());
    assert!(Muxer::new(PPS, SPS, 640, 360).is_err());
    assert!(Muxer::new(&[0x67, 100, 0, 40], PPS, 640, 360).is_err()); // missing high-profile details
}
