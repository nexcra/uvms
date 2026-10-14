//! RTCP Sender Report A/V clock correlation (RFC 3550 §6.4.1).
//! Video remains the presentation reference. Audio correction slews rather than
//! stepping timestamps, so a late first report cannot send the timeline backwards.
#[derive(Default)]
pub(crate) struct ClockSync {
    video: Track,
    audio: Track,
    correction: i64,
    target: Option<i64>,
    last_audio: Option<u64>,
}
#[derive(Default)]
struct Track {
    last: Option<(u32, u32, u64, u32)>,
    offset: Option<i128>,
}
impl ClockSync {
    pub fn rtp(&mut self, audio: bool, ssrc: u32, rtp: u32, pts: u64, rate: u32) {
        let track = if audio {
            &mut self.audio
        } else {
            &mut self.video
        };
        if track.last.is_some_and(|v| v.0 != ssrc) {
            track.offset = None;
            self.target = None;
        }
        if track
            .last
            .is_some_and(|v| v.0 == ssrc && (rtp.wrapping_sub(v.1) as i32) < 0)
        {
            return;
        }
        track.last = Some((ssrc, rtp, pts, rate));
    }
    pub fn report(&mut self, audio: bool, bytes: &[u8]) {
        let track = if audio {
            &mut self.audio
        } else {
            &mut self.video
        };
        let Some((ssrc, last_rtp, last_pts, rate)) = track.last else {
            return;
        };
        let mut at = 0;
        while at + 4 <= bytes.len() {
            let header = &bytes[at..];
            if header[0] >> 6 != 2 {
                return;
            }
            let length = (usize::from(u16::from_be_bytes([header[2], header[3]])) + 1) * 4;
            if length < 4 || at + length > bytes.len() {
                return;
            }
            if header[1] == 200
                && length >= 28 + 24 * usize::from(header[0] & 31)
                && header[0] & 32 == 0
            {
                let report_ssrc = u32::from_be_bytes(header[4..8].try_into().unwrap());
                let seconds = u32::from_be_bytes(header[8..12].try_into().unwrap());
                let fraction = u32::from_be_bytes(header[12..16].try_into().unwrap());
                let rtp = u32::from_be_bytes(header[16..20].try_into().unwrap());
                let delta = i64::from(rtp.wrapping_sub(last_rtp) as i32);
                // SRs too far from the latest media packet are stale or belong to
                // a restarted clock. Do not turn them into an arbitrary seek.
                if report_ssrc == ssrc && seconds != 0 && delta.abs() <= i64::from(rate) * 10 {
                    let ntp = i128::from(seconds) * 90000 + (i128::from(fraction) * 90000 >> 32);
                    let pts = i128::from(last_pts) + i128::from(delta) * 90000 / i128::from(rate);
                    track.offset = Some(ntp - pts);
                }
            }
            at += length;
        }
        if let (Some(video), Some(audio)) = (self.video.offset, self.audio.offset) {
            let delta = audio - video;
            // Ordinary network skew / camera clock drift is bounded. A larger
            // disagreement requires reconnect/diagnosis, not an inaudible jump.
            if delta.abs() <= 450000 {
                self.target = Some(delta as i64);
            }
        }
    }
    pub fn adjust(&mut self, pts: u64, duration_90k: u64) -> u64 {
        if let Some(target) = self.target {
            let step = (duration_90k / 4).clamp(1, 90) as i64;
            self.correction += (target - self.correction).clamp(-step, step);
        }
        let corrected =
            (i128::from(pts) + i128::from(self.correction)).clamp(0, i128::from(u64::MAX)) as u64;
        let corrected = corrected.max(self.last_audio.unwrap_or(0));
        self.last_audio = Some(corrected);
        corrected
    }
    pub fn status(&self) -> (bool, i64) {
        (self.target.is_some(), self.correction)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn report(ssrc: u32, sec: u32, frac: u32, rtp: u32) -> Vec<u8> {
        let mut p = vec![0x80, 200, 0, 6];
        for v in [ssrc, sec, frac, rtp, 0, 0] {
            p.extend(v.to_be_bytes());
        }
        p
    }
    #[test]
    fn sender_reports_remove_arrival_skew_without_backward_step() {
        let mut sync = ClockSync::default();
        sync.rtp(false, 1, 900000, 90000, 90000);
        sync.rtp(true, 2, 480000, 99000, 48000);
        sync.report(false, &report(1, 1000, 0, 900000));
        sync.report(true, &report(2, 1000, 0, 480000));
        let mut previous = 0;
        for n in 0..150 {
            let next = sync.adjust(99000 + n * 1800, 1800);
            assert!(next > previous);
            previous = next;
        }
        assert_eq!(sync.status(), (true, -9000));
        assert_eq!(previous, 99000 + 149 * 1800 - 9000);
    }
    #[test]
    fn rtp_wrap_compound_sr_bad_ssrc_and_outliers() {
        let mut sync = ClockSync::default();
        sync.rtp(false, 1, u32::MAX - 8999, 90000, 90000);
        sync.rtp(true, 2, u32::MAX - 4799, 90000, 48000);
        let mut compound = vec![0x80, 201, 0, 1, 0, 0, 0, 1];
        compound.extend(report(1, 1000, 0, 0));
        sync.report(false, &compound);
        sync.report(true, &report(9, 1000, 0, 0));
        assert!(!sync.status().0);
        sync.report(true, &report(2, 1000, 0, 0));
        assert!(sync.status().0);
        assert_eq!(sync.adjust(90000, 1800), 90000);
        sync.report(true, &report(2, 1100, 0, 0));
        assert_eq!(sync.adjust(91800, 1800), 91800);
    }
    #[test]
    fn thirty_second_audio_clock_drift_is_corrected() {
        let mut sync = ClockSync::default();
        let mut last = 0;
        for second in 0..=30u32 {
            // Audio advances 50ppm faster; separate raw RTP epochs are irrelevant.
            let audio_ticks = second * 48000 + second * 24 / 10;
            sync.rtp(
                false,
                1,
                12345 + second * 90000,
                u64::from(second) * 90000,
                90000,
            );
            sync.rtp(
                true,
                2,
                54321 + audio_ticks,
                u64::from(audio_ticks) * 90000 / 48000,
                48000,
            );
            sync.report(false, &report(1, 1000 + second, 0, 12345 + second * 90000));
            sync.report(true, &report(2, 1000 + second, 0, 54321 + audio_ticks));
            for frame in 0..50 {
                last = sync.adjust(u64::from(audio_ticks) * 90000 / 48000 + frame * 1800, 1800);
            }
        }
        assert!(last.abs_diff(30 * 90000 + 49 * 1800) <= 1);
    }
}
