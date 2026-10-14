use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    net::{IpAddr, SocketAddr},
};
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub bind: SocketAddr,
    pub rtsp_bind: SocketAddr,
    pub default_stream: Option<String>,
    pub web_root: String,
    pub max_sessions: usize,
    pub ice_udp_min: u16,
    pub ice_udp_max: u16,
    pub public_ip: Option<IpAddr>,
    pub ice_servers: Vec<String>,
    pub recording_dir: String,
    pub segment_seconds: u64,
    pub recording_limit_bytes: u64,
    pub hls_segment_seconds: u64,
    pub hls_window_seconds: u64,
    pub hls_limit_bytes: u64,
    pub hls_idle_seconds: u64,
    pub nvidia_device: u32,
    pub nvidia_bitrate: u32,
    pub streams: BTreeMap<String, StreamConfig>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:8080".parse().unwrap(),
            rtsp_bind: "127.0.0.1:8555".parse().unwrap(),
            default_stream: None,
            web_root: "web".into(),
            max_sessions: 32,
            ice_udp_min: 50000,
            ice_udp_max: 50100,
            public_ip: None,
            ice_servers: vec![],
            recording_dir: "recordings".into(),
            segment_seconds: 60,
            recording_limit_bytes: 10 * 1024 * 1024 * 1024,
            hls_segment_seconds: 2,
            hls_window_seconds: 300,
            hls_limit_bytes: 128 * 1024 * 1024,
            hls_idle_seconds: 60,
            nvidia_device: 0,
            nvidia_bitrate: 4_000_000,
            streams: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct StreamConfig {
    pub url_env: String,
    pub record: bool,
}
impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.nvidia_device > 63 || !(100_000..=100_000_000).contains(&self.nvidia_bitrate) {
            bail!("invalid NVIDIA device/bitrate")
        }
        if !(1..=10).contains(&self.hls_segment_seconds)
            || self.hls_window_seconds < 3 * self.hls_segment_seconds
            || self.hls_window_seconds > 86400
            || !(1024 * 1024..=1024 * 1024 * 1024).contains(&self.hls_limit_bytes)
            || !(15..=86400).contains(&self.hls_idle_seconds)
        {
            bail!("invalid HLS cache limits")
        }
        if self
            .default_stream
            .as_ref()
            .is_some_and(|id| !self.streams.contains_key(id))
        {
            bail!("default_stream must name a configured stream")
        }
        if !(1..=1024).contains(&self.max_sessions) {
            bail!("max_sessions must be 1..1024")
        }
        if self.ice_udp_min < 1024 || self.ice_udp_max < self.ice_udp_min {
            bail!("invalid ICE UDP range")
        }
        if self.public_ip.is_some_and(|ip| !ip.is_ipv4()) {
            bail!("public_ip must be IPv4")
        }
        if !(2..=86400).contains(&self.segment_seconds) || self.recording_limit_bytes < 1024 * 1024
        {
            bail!("invalid recording limits")
        }
        if self.streams.is_empty() {
            bail!("configure at least one stream")
        }
        for (id, s) in &self.streams {
            if !valid_id(id) || s.url_env.is_empty() {
                bail!("invalid stream id or missing url_env")
            }
        }
        Ok(())
    }
}
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}
pub fn source_url(s: &StreamConfig) -> Result<String> {
    let value = std::env::var(&s.url_env).context("camera URL environment variable missing")?;
    let u = url::Url::parse(&value).map_err(|_| anyhow::anyhow!("invalid camera URL"))?;
    if u.scheme() != "rtsp" || u.host_str().is_none() || value.contains(['\r', '\n', '\0']) {
        bail!("source requires rtsp:// URL (TCP interleaved)")
    }
    Ok(value)
}
