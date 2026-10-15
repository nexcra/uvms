mod discovery;
mod soap;
#[cfg(test)]
mod tests;
use crate::App;
use anyhow::{ensure, Context, Result};
use axum::{
    extract::{Path, State as AxumState},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use soap::{child, parse, text, xml, Client, DEVICE, MEDIA, MEDIA2, PTZ, SCHEMA};
use std::{
    collections::HashMap,
    net::Ipv4Addr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};

pub struct State {
    cameras: Mutex<HashMap<String, Arc<Camera>>>,
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    discovery: Mutex<()>,
    queries: Semaphore,
}
impl Default for State {
    fn default() -> Self {
        Self {
            cameras: Mutex::default(),
            locks: Mutex::default(),
            discovery: Mutex::new(()),
            queries: Semaphore::new(4),
        }
    }
}
struct Camera {
    client: Client,
    device_url: String,
    ptz_url: Option<String>,
    media_ns: &'static str,
    profiles: Vec<Profile>,
    identity: serde_json::Value,
    created: Instant,
    options: Mutex<HashMap<String, Options>>,
    movement: Arc<Mutex<()>>,
}
#[derive(Clone, Serialize)]
struct Profile {
    token: String,
    name: String,
    ptz_configuration: Option<String>,
}
#[derive(Clone, Serialize, Default)]
struct Options {
    pan_tilt: Option<Space2>,
    zoom: Option<Space1>,
    min_duration_ms: u64,
    max_duration_ms: u64,
}
#[derive(Clone, Serialize)]
struct Space2 {
    uri: String,
    x: Range,
    y: Range,
}
#[derive(Clone, Serialize)]
struct Space1 {
    uri: String,
    x: Range,
}
#[derive(Clone, Serialize)]
struct Range {
    min: f64,
    max: f64,
}
fn credentials(app: &App, id: &str) -> Result<(url::Url, String, String)> {
    let config = app.config.streams.get(id).context("unknown camera")?;
    let source = url::Url::parse(&crate::config::source_url(config)?)
        .map_err(|_| anyhow::anyhow!("invalid camera source"))?;
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8()
            .map(|s| s.into_owned())
            .map_err(|_| anyhow::anyhow!("invalid camera credential encoding"))
    };
    let user = match &config.onvif_username_env {
        Some(name) => std::env::var(name).context("ONVIF username environment missing")?,
        None => decode(source.username())?,
    };
    let password = match &config.onvif_password_env {
        Some(name) => std::env::var(name).context("ONVIF password environment missing")?,
        None => decode(source.password().unwrap_or(""))?,
    };
    Ok((source, user, password))
}
fn default_endpoint(source: &url::Url) -> Result<String> {
    let mut url = url::Url::parse("http://127.0.0.1/onvif/device_service")?;
    url.set_host(source.host_str())
        .map_err(|_| anyhow::anyhow!("invalid camera host"))?;
    Ok(url.into())
}
fn failure(error: anyhow::Error) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({"error":error.to_string()})),
    )
        .into_response()
}
fn denied() -> Response {
    StatusCode::FORBIDDEN.into_response()
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryRequest {
    #[serde(default)]
    interface: Option<Ipv4Addr>,
}
pub async fn discover(
    AxumState(app): AxumState<Arc<App>>,
    headers: HeaderMap,
    Json(request): Json<DiscoveryRequest>,
) -> Response {
    if !crate::same_origin(&headers) {
        return denied();
    }
    let Ok(_guard) = app.onvif.discovery.try_lock() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let known = app
        .config
        .streams
        .iter()
        .filter_map(|(id, c)| {
            let source = url::Url::parse(&crate::config::source_url(c).ok()?).ok()?;
            Some((id.clone(), source.host_str()?.to_owned()))
        })
        .collect::<Vec<_>>();
    match discovery::discover(request.interface.unwrap_or(Ipv4Addr::UNSPECIFIED), &known).await {
        Ok(devices) => {
            Json(json!({"devices":devices,"scan_seconds":3,"ipv4_only":true})).into_response()
        }
        Err(_) => failure(anyhow::anyhow!(
            "ONVIF discovery failed; check the server network interface"
        )),
    }
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    #[serde(default)]
    device_url: Option<String>,
}
pub async fn status(AxumState(app): AxumState<Arc<App>>, Path(id): Path<String>) -> Response {
    if !app.config.streams.contains_key(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let cameras = app.onvif.cameras.lock().await;
    if let Some(camera) = cameras.get(&id) {
        return camera_info(camera).into_response();
    }
    drop(cameras);
    match address(&app, &id, None).await {
        Ok((address, _)) => {
            Json(json!({"ready":false,"device_url":address,"profiles":[]})).into_response()
        }
        Err(e) => failure(e),
    }
}
fn camera_info(camera: &Camera) -> Json<serde_json::Value> {
    Json(
        json!({"ready":camera.created.elapsed()<Duration::from_secs(600),"device_url":camera.device_url,"media_service":camera.media_ns,"identity":camera.identity,"profiles":camera.profiles,"ptz_service":camera.ptz_url.is_some(),"clock_offset_ms":camera.client.offset_ms}),
    )
}
async fn address(app: &App, id: &str, explicit: Option<String>) -> Result<(String, Client)> {
    let (source, user, password) = credentials(app, id)?;
    let configured = app.config.streams[id].onvif_url.clone();
    let saved = app.archive.onvif_endpoint(id).await?;
    let address = explicit
        .or(configured)
        .or(saved)
        .unwrap_or(default_endpoint(&source)?);
    let host = source.host_str().context("missing camera host")?;
    let address = soap::endpoint(&address, host)?.to_string();
    Ok((address, Client::new(host.into(), user, password)?))
}
pub async fn inspect(
    AxumState(app): AxumState<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<InspectRequest>,
) -> Response {
    if !crate::same_origin(&headers) {
        return denied();
    }
    if !app.config.streams.contains_key(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(_permit) = app.onvif.queries.try_acquire() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    match inspect_camera(&app, &id, request.device_url).await {
        Ok(camera) => {
            let info = camera_info(&camera);
            app.onvif
                .cameras
                .lock()
                .await
                .insert(id.clone(), Arc::new(camera));
            app.events.emit(Some(&id), "onvif", "inspected");
            info.into_response()
        }
        Err(e) => failure(e),
    }
}
async fn inspect_camera(app: &App, id: &str, explicit: Option<String>) -> Result<Camera> {
    let (device_url, mut client) = address(app, id, explicit).await?;
    client.synchronize(&device_url).await;
    let services = client
        .call(
            &device_url,
            DEVICE,
            "GetServices",
            "<t:IncludeCapability>false</t:IncludeCapability>",
            true,
        )
        .await;
    let mut media = None;
    let mut media2 = None;
    let mut ptz = None;
    if let Ok(response) = services {
        let doc = parse(&response)?;
        for node in doc
            .descendants()
            .filter(|n| n.has_tag_name((DEVICE, "Service")))
        {
            let Some(ns) = text(node, DEVICE, "Namespace") else {
                continue;
            };
            let Some(raw) = text(node, DEVICE, "XAddr") else {
                continue;
            };
            if ![MEDIA, MEDIA2, PTZ].contains(&ns.as_str()) {
                continue;
            }
            let url = soap::endpoint(&raw, &client.host)?.to_string();
            match ns.as_str() {
                MEDIA => media = Some(url),
                MEDIA2 => media2 = Some(url),
                PTZ => ptz = Some(url),
                _ => {}
            }
        }
    } else {
        let response = client
            .call(
                &device_url,
                DEVICE,
                "GetCapabilities",
                "<t:Category>All</t:Category>",
                true,
            )
            .await?;
        let doc = parse(&response)?;
        for (name, dest) in [("Media", &mut media), ("PTZ", &mut ptz)] {
            if let Some(node) = doc.descendants().find(|n| n.has_tag_name((SCHEMA, name))) {
                if let Some(raw) = text(node, SCHEMA, "XAddr") {
                    *dest = Some(soap::endpoint(&raw, &client.host)?.to_string());
                }
            }
        }
    }
    let (media_url, media_ns) = if let Some(url) = media {
        (url, MEDIA)
    } else {
        (
            media2.context("camera exposes no supported ONVIF media service")?,
            MEDIA2,
        )
    };
    let response = client
        .call(
            &media_url,
            media_ns,
            "GetProfiles",
            if media_ns == MEDIA2 {
                "<t:Type>All</t:Type>"
            } else {
                ""
            },
            true,
        )
        .await?;
    let profiles = parse_profiles(&response, media_ns)?;
    let identity = match client
        .call(&device_url, DEVICE, "GetDeviceInformation", "", true)
        .await
    {
        Ok(response) => {
            let doc = parse(&response)?;
            let node = doc
                .descendants()
                .find(|n| n.has_tag_name((DEVICE, "GetDeviceInformationResponse")))
                .context("missing device information")?;
            json!({"manufacturer":text(node,DEVICE,"Manufacturer"),"model":text(node,DEVICE,"Model"),"firmware":text(node,DEVICE,"FirmwareVersion")})
        }
        Err(_) => json!(null),
    };
    let movement = app
        .onvif
        .locks
        .lock()
        .await
        .entry(client.host.clone())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();
    app.archive.save_onvif_endpoint(id, &device_url).await?;
    Ok(Camera {
        client,
        device_url,
        ptz_url: ptz,
        media_ns,
        profiles,
        identity,
        created: Instant::now(),
        options: Mutex::default(),
        movement,
    })
}
fn parse_profiles(response: &str, ns: &str) -> Result<Vec<Profile>> {
    let doc = parse(response)?;
    let mut profiles = vec![];
    for node in doc
        .descendants()
        .filter(|n| n.has_tag_name((ns, "Profiles")))
        .take(64)
    {
        let token = node
            .attribute("token")
            .filter(|s| !s.is_empty() && s.len() <= 512)
            .context("invalid ONVIF profile token")?
            .to_owned();
        let name = text(node, SCHEMA, "Name")
            .or_else(|| text(node, ns, "Name"))
            .unwrap_or_else(|| token.clone());
        let ptz = child(node, SCHEMA, "PTZConfiguration")
            .or_else(|| child(node, ns, "Configurations").and_then(|n| child(n, ns, "PTZ")));
        profiles.push(Profile {
            token,
            name,
            ptz_configuration: ptz
                .and_then(|n| n.attribute("token"))
                .filter(|s| s.len() <= 512)
                .map(str::to_owned),
        });
    }
    ensure!(!profiles.is_empty(), "camera returned no ONVIF profiles");
    Ok(profiles)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileRequest {
    profile_token: String,
}
async fn camera(app: &App, id: &str) -> Result<Arc<Camera>> {
    let result = app
        .onvif
        .cameras
        .lock()
        .await
        .get(id)
        .cloned()
        .context("inspect this ONVIF camera first")?;
    ensure!(
        result.created.elapsed() < Duration::from_secs(600),
        "ONVIF information expired; inspect the camera again"
    );
    Ok(result)
}
fn profile<'a>(camera: &'a Camera, token: &str) -> Result<&'a Profile> {
    camera
        .profiles
        .iter()
        .find(|p| p.token == token)
        .context("unknown ONVIF profile")
}
pub async fn details(
    AxumState(app): AxumState<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(request): Json<ProfileRequest>,
) -> Response {
    if !crate::same_origin(&headers) {
        return denied();
    }
    let Ok(_permit) = app.onvif.queries.try_acquire() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    match profile_details(&app, &id, &request.profile_token).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => failure(e),
    }
}
async fn profile_details(app: &App, id: &str, token: &str) -> Result<serde_json::Value> {
    let camera = camera(app, id).await?;
    let profile = profile(&camera, token)?;
    let Some((ptz, config)) = camera
        .ptz_url
        .as_ref()
        .zip(profile.ptz_configuration.as_ref())
    else {
        return Ok(
            json!({"profile_token":token,"supported":false,"reason":"profile_has_no_ptz","options":Options::default()}),
        );
    };
    let response = camera
        .client
        .call(
            ptz,
            PTZ,
            "GetConfigurationOptions",
            &format!(
                "<t:ConfigurationToken>{}</t:ConfigurationToken>",
                xml(config)
            ),
            true,
        )
        .await?;
    let options = parse_options(&response)?;
    let response = camera
        .client
        .call(
            ptz,
            PTZ,
            "GetStatus",
            &format!("<t:ProfileToken>{}</t:ProfileToken>", xml(token)),
            true,
        )
        .await;
    let status = response.ok().and_then(|s| {
        let doc = parse(&s).ok()?;
        let node = doc
            .descendants()
            .find(|n| n.has_tag_name((SCHEMA, "MoveStatus")))?;
        Some(json!({"pan_tilt":text(node,SCHEMA,"PanTilt"),"zoom":text(node,SCHEMA,"Zoom")}))
    });
    camera
        .options
        .lock()
        .await
        .insert(token.into(), options.clone());
    Ok(
        json!({"profile_token":token,"supported":options.pan_tilt.is_some()||options.zoom.is_some(),"options":options,"status":status}),
    )
}
fn duration_ms(value: &str) -> Option<u64> {
    let mut total = 0f64;
    let mut number = String::new();
    for ch in value.strip_prefix("PT")?.chars() {
        if ch.is_ascii_digit() || ch == '.' {
            number.push(ch);
            continue;
        }
        let n = number.parse::<f64>().ok()?;
        number.clear();
        total += n * match ch {
            'H' => 3600.,
            'M' => 60.,
            'S' => 1.,
            _ => return None,
        };
    }
    if !number.is_empty() || !total.is_finite() || total < 0. || total > 86400. {
        return None;
    }
    Some((total * 1000.).round() as u64)
}
fn parse_options(response: &str) -> Result<Options> {
    let doc = parse(response)?;
    let range = |node, name| -> Option<Range> {
        let node = child(node, SCHEMA, name)?;
        let min = text(node, SCHEMA, "Min")?.parse::<f64>().ok()?;
        let max = text(node, SCHEMA, "Max")?.parse::<f64>().ok()?;
        if min.is_finite() && max.is_finite() && min <= 0. && max >= 0. && min < max {
            Some(Range { min, max })
        } else {
            None
        }
    };
    let pan_tilt = doc
        .descendants()
        .filter(|n| n.has_tag_name((SCHEMA, "ContinuousPanTiltVelocitySpace")))
        .find_map(|n| {
            Some(Space2 {
                uri: text(n, SCHEMA, "URI")?,
                x: range(n, "XRange")?,
                y: range(n, "YRange")?,
            })
        });
    let zoom = doc
        .descendants()
        .filter(|n| n.has_tag_name((SCHEMA, "ContinuousZoomVelocitySpace")))
        .find_map(|n| {
            Some(Space1 {
                uri: text(n, SCHEMA, "URI")?,
                x: range(n, "XRange")?,
            })
        });
    let timeout = doc
        .descendants()
        .find(|n| n.has_tag_name((SCHEMA, "PTZTimeout")));
    let min = timeout
        .and_then(|n| text(n, SCHEMA, "Min"))
        .and_then(|s| duration_ms(&s));
    let max = timeout
        .and_then(|n| text(n, SCHEMA, "Max"))
        .and_then(|s| duration_ms(&s));
    let (Some(min), Some(max)) = (min, max) else {
        return Ok(Options::default());
    };
    let min = min.max(100);
    let max = max.min(2000);
    if min > max {
        return Ok(Options::default());
    }
    Ok(Options {
        pan_tilt,
        zoom,
        min_duration_ms: min,
        max_duration_ms: max,
    })
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Move {
        profile_token: String,
        #[serde(default)]
        pan: f64,
        #[serde(default)]
        tilt: f64,
        #[serde(default)]
        zoom: f64,
        duration_ms: u64,
    },
    Stop {
        profile_token: String,
    },
}
fn movement_body(
    token: &str,
    pan: f64,
    tilt: f64,
    zoom: f64,
    duration: u64,
    options: &Options,
) -> Result<String> {
    ensure!(
        [pan, tilt, zoom]
            .iter()
            .all(|v| v.is_finite() && (-1.0..=1.0).contains(v)),
        "PTZ velocity must be between -1 and 1"
    );
    ensure!(
        duration >= options.min_duration_ms
            && duration <= options.max_duration_ms
            && duration >= 100,
        "PTZ duration is outside camera limits"
    );
    ensure!(
        pan != 0. || tilt != 0. || zoom != 0.,
        "PTZ move requires a nonzero velocity"
    );
    let scaled = |v: f64, r: &Range| if v >= 0. { v * r.max } else { -v * r.min };
    let mut velocity = String::new();
    if pan != 0. || tilt != 0. {
        let space = options
            .pan_tilt
            .as_ref()
            .context("profile does not support pan/tilt")?;
        velocity.push_str(&format!(
            r#"<tt:PanTilt x="{}" y="{}" space="{}"/>"#,
            scaled(pan, &space.x),
            scaled(tilt, &space.y),
            xml(&space.uri)
        ));
    }
    if zoom != 0. {
        let space = options
            .zoom
            .as_ref()
            .context("profile does not support zoom")?;
        velocity.push_str(&format!(
            r#"<tt:Zoom x="{}" space="{}"/>"#,
            scaled(zoom, &space.x),
            xml(&space.uri)
        ));
    }
    Ok(format!("<t:ProfileToken>{}</t:ProfileToken><t:Velocity>{velocity}</t:Velocity><t:Timeout>PT{:.3}S</t:Timeout>",xml(token),duration as f64/1000.))
}
async fn stop(camera: &Camera, token: &str) -> Result<()> {
    let address = camera
        .ptz_url
        .as_ref()
        .context("camera has no PTZ service")?;
    let body = format!(
        "<t:ProfileToken>{}</t:ProfileToken><t:PanTilt>true</t:PanTilt><t:Zoom>true</t:Zoom>",
        xml(token)
    );
    camera
        .client
        .call(address, PTZ, "Stop", &body, true)
        .await?;
    Ok(())
}
pub async fn command(
    AxumState(app): AxumState<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(command): Json<Command>,
) -> Response {
    if !crate::same_origin(&headers) {
        return denied();
    }
    let result = execute(app, id, command).await;
    match result {
        Ok(()) => Json(json!({"status":"stopped"})).into_response(),
        Err(e) => failure(e),
    }
}
async fn execute(app: Arc<App>, id: String, command: Command) -> Result<()> {
    let camera = camera(&app, &id).await?;
    let token = match &command {
        Command::Move { profile_token, .. } | Command::Stop { profile_token } => {
            profile_token.clone()
        }
    };
    ensure!(
        profile(&camera, &token)?.ptz_configuration.is_some(),
        "profile has no PTZ configuration"
    );
    match command {
        Command::Stop { .. } => {
            stop(&camera, &token).await?;
            app.events.emit(Some(&id), "ptz", "stopped");
            Ok(())
        }
        Command::Move {
            pan,
            tilt,
            zoom,
            duration_ms,
            ..
        } => {
            let options = camera
                .options
                .lock()
                .await
                .get(&token)
                .cloned()
                .context("read this profile's PTZ capabilities first")?;
            let body = movement_body(&token, pan, tilt, zoom, duration_ms, &options)?;
            let lock = camera
                .movement
                .clone()
                .try_lock_owned()
                .map_err(|_| anyhow::anyhow!("camera PTZ is busy"))?;
            // Detached from the HTTP caller: always attempt Stop, even when the
            // browser disappears or the move response times out.
            tokio::spawn(async move {
                let _lock = lock;
                let address = camera
                    .ptz_url
                    .as_ref()
                    .context("camera has no PTZ service")?;
                let moved = camera
                    .client
                    .call(address, PTZ, "ContinuousMove", &body, true)
                    .await;
                if moved.is_ok() {
                    app.events.emit(Some(&id), "ptz", "moving");
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(duration_ms)) => {},
                        _ = app.shutdown.cancelled() => {},
                    }
                }
                let stopped = stop(&camera, &token).await;
                let state = if stopped.is_ok() {
                    "stopped"
                } else {
                    "stop_failed"
                };
                app.events.emit(Some(&id), "ptz", state);
                stopped?;
                moved?;
                Ok::<_, anyhow::Error>(())
            })
            .await
            .context("PTZ command task ended")?
        }
    }
}
