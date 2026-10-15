mod archive;
mod audio;
mod config;
mod hls;
mod ingest;
mod mse;
mod operations;
mod record;
mod record_input;
mod relay;
mod rtc;
mod rtp_video;
mod timeline;
mod video;
use anyhow::{Context, Result};
use axum::{
    extract::{Path, State, WebSocketUpgrade},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use config::Config;
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;
pub(crate) struct App {
    config: Arc<Config>,
    events: operations::Events,
    slots: Arc<Semaphore>,
    hub: ingest::Hub,
    shutdown: CancellationToken,
    recordings: record::Status,
    archive: archive::Archive,
    hls: hls::State,
    rtc_udp: tokio::sync::OnceCell<Arc<webrtc::ice::udp_mux::UDPMuxDefault>>,
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "uvms_gateway=info,webrtc=error".into()),
        )
        .init();
    let path = std::env::var("UVMS_CONFIG").unwrap_or_else(|_| "config.toml".into());
    let cfg: Config = toml::from_str(
        &tokio::fs::read_to_string(path)
            .await
            .context("read UVMS_CONFIG")?,
    )?;
    cfg.validate()?;
    for s in cfg.streams.values() {
        config::source_url(s)?;
    }
    let http = tokio::net::TcpListener::bind(cfg.bind).await?;
    let rtsp = tokio::net::TcpListener::bind(cfg.rtsp_bind).await?;
    let events = operations::Events::default();
    let app = Arc::new(App {
        slots: Arc::new(Semaphore::new(cfg.max_sessions)),
        config: Arc::new(cfg),
        hub: ingest::Hub::new(events.clone()),
        events,
        shutdown: CancellationToken::new(),
        recordings: record::Status::default(),
        archive: archive::Archive::default(),
        hls: hls::State::default(),
        rtc_udp: tokio::sync::OnceCell::new(),
    });
    rtc::shared_udp(&app).await?;
    let recorders = record::start(app.clone()).await?;
    let journal = app.events.start(app.clone()).await?;
    app.events.emit(None, "service", "started");
    let relay = tokio::spawn(relay::run(rtsp, app.clone()));
    tracing::info!(http=%app.config.bind,rtsp=%app.config.rtsp_bind,"Rust media service ready; native H264 passthrough");
    let signal = app.shutdown.clone();
    axum::serve(http, router(app.clone()))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            signal.cancel();
        })
        .await?;
    app.shutdown.cancel();
    for task in recorders {
        let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
    }
    let _ = relay.await;
    rtc::close_udp(&app).await;
    app.events.emit(None, "service", "stopped");
    app.events.stop();
    let _ = tokio::time::timeout(Duration::from_secs(10), journal).await;
    Ok(())
}
fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/healthz",get(||async{Json(json!({"status":"ok","version":env!("CARGO_PKG_VERSION"),"media_engine":"rust-native"}))}))
        .route("/readyz",get(operations::ready))
        .route("/api/status",get(operations::status))
        .route("/api/events",get(operations::events))
        .route("/api/timeline/{stream}",get(timeline::get))
        .route("/api/streams",get(streams))
        .route("/api/gpu",get(|State(app):State<Arc<App>>|async move {Json(video::status(app.config.nvidia_device,app.config.nvidia_bitrate).await)}))
        .route("/api/recordings",get(record::list))
        .route("/api/recording/{stream}",get(record::control_status).post(record::control))
        .route("/api/hls/{stream}",get(hls::status))
        .route("/api/hls/{stream}/keepalive",axum::routing::post(hls::keepalive))
        .route("/hls/{stream}/index.m3u8",get(hls::playlist))
        .route("/hls/{stream}/{file}",get(hls::media))
        .route("/api/recordings/{stream}/{file}",get(record::download))
        .route("/webrtc_playnow/{mode}/{transport}/{stream}",get(upgrade_rtc))
        .route("/{stream}",get(upgrade_mse))
        .fallback_service(ServeDir::new(&app.config.web_root).append_index_html_on_directories(true))
        .layer(tower_http::set_header::SetResponseHeaderLayer::if_not_present(header::CACHE_CONTROL,HeaderValue::from_static("no-store")))
        .with_state(app)
}
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    let host = headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    origin
        .to_str()
        .ok()
        .and_then(|s| url::Url::parse(s).ok())
        .is_some_and(|u| {
            let authority = match u.port() {
                Some(p) => format!("{}:{p}", u.host_str().unwrap_or("")),
                None => u.host_str().unwrap_or("").to_owned(),
            };
            authority == host && matches!(u.scheme(), "http" | "https")
        })
}
async fn streams(State(app): State<Arc<App>>) -> Response {
    let mut streams = vec![];
    for (id, s) in &app.config.streams {
        streams.push(json!({"id":id,"mode":"native","online":app.hub.online(id).await,"record":s.record,"recording":app.recordings.get(id).await,"audio":app.hub.audio_status(id).await,"video":app.hub.video_status(id).await}));
    }
    Json(json!({"streams":streams,"video_only":false,"rtsp_port":app.config.rtsp_bind.port(),"default_stream":app.config.default_stream,"capabilities":{"hls":true,"dvr":true,"recording_control":true,"recording_search":true,"audio":true,"events":true,"timeline":true,"operational_status":true}}))
        .into_response()
}
async fn upgrade_rtc(
    State(app): State<Arc<App>>,
    Path((mode, transport, id)): Path<(String, String, String)>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !matches!(mode.as_str(), "singleport" | "randomport") || transport != "udp" {
        return (
            StatusCode::NOT_IMPLEMENTED,
            "WebRTC supports UDP on the configured shared port",
        )
            .into_response();
    }
    if !app.config.streams.contains_key(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(permit) = app.slots.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let Ok(feed) = app
        .hub
        .acquire(&id, &app.config, app.shutdown.clone())
        .await
    else {
        return (StatusCode::BAD_GATEWAY, "camera unavailable").into_response();
    };
    let Ok(udp) = rtc::shared_udp(&app).await else {
        return (StatusCode::SERVICE_UNAVAILABLE, "WebRTC UDP unavailable").into_response();
    };
    ws.max_message_size(128 * 1024).on_upgrade(move |socket| {
        rtc::serve(
            socket,
            feed,
            app.config.clone(),
            permit,
            app.shutdown.clone(),
            udp,
        )
    })
}
async fn upgrade_mse(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    ws: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    if id == "app.js" || id == "style.css" {
        let path = std::path::Path::new(&app.config.web_root).join(&id);
        return match tokio::fs::read(path).await {
            Ok(b) => (
                [(
                    header::CONTENT_TYPE,
                    if id.ends_with("js") {
                        "text/javascript"
                    } else {
                        "text/css"
                    },
                )],
                b,
            )
                .into_response(),
            Err(_) => StatusCode::NOT_FOUND.into_response(),
        };
    }
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !app.config.streams.contains_key(&id) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(ws) = ws else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(permit) = app.slots.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    ws.max_message_size(4096)
        .on_upgrade(move |socket| mse::serve(socket, app, id, permit))
}
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut term =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).unwrap();
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    pub(crate) fn app() -> Arc<App> {
        Arc::new(App {
            config: Arc::new(Config::default()),
            events: operations::Events::default(),
            slots: Arc::new(Semaphore::new(2)),
            hub: ingest::Hub::default(),
            shutdown: CancellationToken::new(),
            recordings: record::Status::default(),
            archive: archive::Archive::default(),
            hls: hls::State::default(),
            rtc_udp: tokio::sync::OnceCell::new(),
        })
    }

    #[tokio::test]
    async fn web_apis_need_no_credentials_or_session_cookie() {
        let state = app();
        let temporary =
            std::env::temp_dir().join(format!("uvms-api-test-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir_all(&temporary).await.unwrap();
        state
            .archive
            .initialize(temporary.to_str().unwrap())
            .await
            .unwrap();
        for path in ["/api/streams", "/api/recordings"] {
            let response = router(state.clone())
                .oneshot(
                    Request::builder()
                        .uri(path)
                        .header(header::HOST, "media.lan:8080")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(!response.headers().contains_key(header::SET_COOKIE));
        }
        let response = router(app())
            .oneshot(
                Request::builder()
                    .uri("/api/recordings/camera/missing.mp4")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        tokio::fs::remove_dir_all(temporary).await.unwrap();
    }

    #[tokio::test]
    async fn rtsp_options_needs_no_authorization() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let app = app();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(relay::run(listener, app.clone()));
        let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
        client
            .write_all(b"OPTIONS * RTSP/1.0\r\nCSeq: 1\r\n\r\n")
            .await
            .unwrap();
        let mut received = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !received.ends_with(b"\r\n\r\n") {
                received.push(client.read_u8().await.unwrap());
            }
        })
        .await
        .unwrap();
        let text = String::from_utf8(received).unwrap();
        assert!(text.starts_with("RTSP/1.0 200 OK"));
        assert!(!text.contains("WWW-Authenticate"));
        app.shutdown.cancel();
        server.await.unwrap();
    }
}
