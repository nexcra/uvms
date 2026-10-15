use anyhow::{bail, ensure, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use roxmltree::{Document, Node};
use sha1::{Digest, Sha1};
use std::time::Duration;
use url::Url;

pub const DEVICE: &str = "http://www.onvif.org/ver10/device/wsdl";
pub const MEDIA: &str = "http://www.onvif.org/ver10/media/wsdl";
pub const MEDIA2: &str = "http://www.onvif.org/ver20/media/wsdl";
pub const PTZ: &str = "http://www.onvif.org/ver20/ptz/wsdl";
pub const SCHEMA: &str = "http://www.onvif.org/ver10/schema";
pub const SOAP: &str = "http://www.w3.org/2003/05/soap-envelope";
pub fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
pub fn parse(text: &str) -> Result<Document<'_>> {
    ensure!(text.len() <= 1024 * 1024, "ONVIF response too large");
    Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 20_000,
            ..Default::default()
        },
    )
    .map_err(|_| anyhow::anyhow!("invalid ONVIF XML"))
}
pub fn child<'a, 'i>(node: Node<'a, 'i>, ns: &str, name: &str) -> Option<Node<'a, 'i>> {
    node.children().find(|n| n.has_tag_name((ns, name)))
}
pub fn text(node: Node<'_, '_>, ns: &str, name: &str) -> Option<String> {
    child(node, ns, name)?.text().map(|v| v.trim().to_owned())
}
pub fn endpoint(raw: &str, host: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| anyhow::anyhow!("invalid ONVIF endpoint"))?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str() == Some(host)
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none(),
        "ONVIF endpoint must use the configured camera host without embedded credentials"
    );
    ensure!(raw.len() <= 2048, "ONVIF endpoint too long");
    Ok(url)
}

pub struct Client {
    http: reqwest::Client,
    pub host: String,
    username: String,
    password: String,
    pub offset_ms: i64,
}
impl Client {
    pub fn new(host: String, username: String, password: String) -> Result<Self> {
        ensure!(
            username.len() <= 1024 && password.len() <= 1024,
            "camera credentials too long"
        );
        Ok(Self {
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(3))
                .timeout(Duration::from_secs(5))
                .build()?,
            host,
            username,
            password,
            offset_ms: 0,
        })
    }
    fn security(&self) -> Result<String> {
        if self.username.is_empty() {
            return Ok(String::new());
        }
        let now = (crate::record::now_ms() as i64)
            .checked_add(self.offset_ms)
            .context("invalid camera time")?;
        let created = chrono::DateTime::from_timestamp_millis(now)
            .context("invalid camera time")?
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let nonce = uuid::Uuid::new_v4();
        let mut hash = Sha1::new();
        hash.update(nonce.as_bytes());
        hash.update(created.as_bytes());
        hash.update(self.password.as_bytes());
        Ok(format!(
            r#"<wsse:Security s:mustUnderstand="1" xmlns:wsse="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd" xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd"><wsse:UsernameToken><wsse:Username>{}</wsse:Username><wsse:Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">{}</wsse:Password><wsse:Nonce EncodingType="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary">{}</wsse:Nonce><wsu:Created>{}</wsu:Created></wsse:UsernameToken></wsse:Security>"#,
            xml(&self.username),
            STANDARD.encode(hash.finalize()),
            STANDARD.encode(nonce.as_bytes()),
            created
        ))
    }
    pub async fn call(
        &self,
        address: &str,
        ns: &str,
        operation: &str,
        inner: &str,
        authenticated: bool,
    ) -> Result<String> {
        let url = endpoint(address, &self.host)?;
        let header = if authenticated {
            self.security()?
        } else {
            String::new()
        };
        let body = format!(
            r#"<?xml version="1.0"?><s:Envelope xmlns:s="{SOAP}" xmlns:tt="{SCHEMA}"><s:Header>{header}</s:Header><s:Body><t:{operation} xmlns:t="{ns}">{inner}</t:{operation}></s:Body></s:Envelope>"#
        );
        let action = format!("{ns}/{operation}");
        let mut authorization = None;
        for attempt in 0..2 {
            let mut request = self
                .http
                .post(url.clone())
                .header(
                    "Content-Type",
                    format!("application/soap+xml; charset=utf-8; action=\"{action}\""),
                )
                .body(body.clone());
            if let Some(value) = authorization.as_ref() {
                request = request.header("Authorization", value);
            }
            let mut response = request
                .send()
                .await
                .map_err(|_| anyhow::anyhow!("ONVIF connection or request timeout"))?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED
                && attempt == 0
                && authenticated
            {
                let challenge = response
                    .headers()
                    .get_all("www-authenticate")
                    .iter()
                    .filter_map(|h| h.to_str().ok())
                    .find(|h| h.to_ascii_lowercase().starts_with("digest "))
                    .context(
                        "camera requires unsupported HTTP authentication or rejected credentials",
                    )?;
                let challenge = unreal_rtsp::auth::Challenge::parse(challenge)
                    .map_err(|_| anyhow::anyhow!("invalid camera HTTP Digest challenge"))?;
                let uri = match url.query() {
                    Some(q) => format!("{}?{q}", url.path()),
                    None => url.path().to_owned(),
                };
                authorization = Some(
                    challenge
                        .authorization(&self.username, &self.password, "POST", &uri, 1)
                        .map_err(|_| {
                            anyhow::anyhow!("unsupported camera HTTP Digest authentication")
                        })?,
                );
                continue;
            }
            let status = response.status();
            let mut data = Vec::new();
            while let Some(chunk) = response
                .chunk()
                .await
                .map_err(|_| anyhow::anyhow!("incomplete ONVIF response"))?
            {
                ensure!(
                    data.len() + chunk.len() <= 1024 * 1024,
                    "ONVIF response too large"
                );
                data.extend_from_slice(&chunk);
            }
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN
            {
                bail!("ONVIF camera authentication failed")
            }
            ensure!(
                status.is_success(),
                "ONVIF camera returned HTTP {}",
                status.as_u16()
            );
            let result = String::from_utf8(data)
                .map_err(|_| anyhow::anyhow!("invalid ONVIF response encoding"))?;
            let doc = parse(&result)?;
            let root = doc.root_element();
            ensure!(
                root.has_tag_name((SOAP, "Envelope")),
                "unexpected ONVIF SOAP envelope"
            );
            let body = child(root, SOAP, "Body").context("missing ONVIF SOAP body")?;
            ensure!(
                child(body, SOAP, "Fault").is_none(),
                "ONVIF SOAP fault: operation unsupported or credentials rejected"
            );
            ensure!(
                child(body, ns, &format!("{operation}Response")).is_some(),
                "unexpected ONVIF response operation"
            );
            return Ok(result);
        }
        bail!("ONVIF camera authentication failed")
    }
    pub async fn synchronize(&mut self, address: &str) {
        let Ok(response) = self
            .call(address, DEVICE, "GetSystemDateAndTime", "", false)
            .await
        else {
            return;
        };
        let Ok(doc) = parse(&response) else { return };
        let Some(utc) = doc
            .descendants()
            .find(|n| n.has_tag_name((SCHEMA, "UTCDateTime")))
        else {
            return;
        };
        let timestamp = (|| {
            let date = child(utc, SCHEMA, "Date")?;
            let time = child(utc, SCHEMA, "Time")?;
            chrono::NaiveDate::from_ymd_opt(
                text(date, SCHEMA, "Year")?.parse().ok()?,
                text(date, SCHEMA, "Month")?.parse().ok()?,
                text(date, SCHEMA, "Day")?.parse().ok()?,
            )?
            .and_hms_opt(
                text(time, SCHEMA, "Hour")?.parse().ok()?,
                text(time, SCHEMA, "Minute")?.parse().ok()?,
                text(time, SCHEMA, "Second")?.parse().ok()?,
            )
            .map(|d| d.and_utc().timestamp_millis())
        })();
        if let Some(timestamp) = timestamp {
            self.offset_ms = timestamp - crate::record::now_ms() as i64;
        }
    }
}
