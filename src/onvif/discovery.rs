use super::soap::{child, parse, text, SOAP};
use anyhow::Result;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
use tokio::net::UdpSocket;
const DISCOVERY: &str = "http://schemas.xmlsoap.org/ws/2005/04/discovery";
const ADDRESSING: &str = "http://schemas.xmlsoap.org/ws/2004/08/addressing";
#[derive(Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub address: String,
    pub name: String,
    pub device_urls: Vec<String>,
    pub matched_streams: Vec<String>,
}
pub fn probe(id: &str) -> String {
    format!(
        r#"<s:Envelope xmlns:s="{SOAP}" xmlns:a="{ADDRESSING}" xmlns:d="{DISCOVERY}" xmlns:dn="http://www.onvif.org/ver10/network/wsdl"><s:Header><a:MessageID>{id}</a:MessageID><a:To>urn:schemas-xmlsoap-org:ws:2005:04:discovery</a:To><a:Action>{DISCOVERY}/Probe</a:Action></s:Header><s:Body><d:Probe><d:Types>dn:NetworkVideoTransmitter</d:Types></d:Probe></s:Body></s:Envelope>"#
    )
}
pub fn matches(xml: &str, request: &str, peer: SocketAddr) -> Result<Vec<Device>> {
    let doc = parse(xml)?;
    let root = doc.root_element();
    anyhow::ensure!(
        root.has_tag_name((SOAP, "Envelope")),
        "invalid discovery envelope"
    );
    let header =
        child(root, SOAP, "Header").ok_or_else(|| anyhow::anyhow!("missing discovery header"))?;
    anyhow::ensure!(
        text(header, ADDRESSING, "RelatesTo").as_deref() == Some(request),
        "unrelated discovery reply"
    );
    let body = child(root, SOAP, "Body")
        .and_then(|n| child(n, DISCOVERY, "ProbeMatches"))
        .ok_or_else(|| anyhow::anyhow!("missing probe matches"))?;
    let mut result = vec![];
    for node in body
        .children()
        .filter(|n| n.has_tag_name((DISCOVERY, "ProbeMatch")))
        .take(128)
    {
        let Some(id) = child(node, ADDRESSING, "EndpointReference")
            .and_then(|n| text(n, ADDRESSING, "Address"))
            .filter(|s| s.len() <= 512)
        else {
            continue;
        };
        let urls = text(node, DISCOVERY, "XAddrs")
            .unwrap_or_default()
            .split_whitespace()
            .take(8)
            .filter_map(|s| {
                let url = super::soap::endpoint(s, &peer.ip().to_string()).ok()?;
                Some(url.to_string())
            })
            .collect::<Vec<_>>();
        if urls.is_empty() {
            continue;
        }
        let scopes = text(node, DISCOVERY, "Scopes").unwrap_or_default();
        let name = scopes
            .split_whitespace()
            .find_map(|s| s.strip_prefix("onvif://www.onvif.org/name/"))
            .map(|s| {
                percent_encoding::percent_decode_str(s)
                    .decode_utf8_lossy()
                    .chars()
                    .take(128)
                    .collect()
            })
            .unwrap_or_else(|| peer.ip().to_string());
        result.push(Device {
            id,
            address: peer.ip().to_string(),
            name,
            device_urls: urls,
            matched_streams: vec![],
        });
    }
    Ok(result)
}
pub async fn discover(interface: Ipv4Addr, known: &[(String, String)]) -> Result<Vec<Device>> {
    let socket = socket2::Socket::new(
        socket2::Domain::IPV4,
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    socket.set_nonblocking(true)?;
    socket.set_multicast_ttl_v4(1)?;
    socket.set_multicast_if_v4(&interface)?;
    socket.bind(&SocketAddr::from((interface, 0)).into())?;
    let udp = UdpSocket::from_std(socket.into())?;
    let id = format!("urn:uuid:{}", uuid::Uuid::new_v4());
    let message = probe(&id);
    let mut targets = vec![SocketAddr::from(([239, 255, 255, 250], 3702))];
    for (_, host) in known.iter().take(64) {
        if let Ok(ip) = host.parse::<Ipv4Addr>() {
            let target = SocketAddr::from((ip, 3702));
            if !targets.contains(&target) {
                targets.push(target)
            }
        }
    }
    let deadline = tokio::time::sleep(Duration::from_secs(3));
    tokio::pin!(deadline);
    let mut retry = tokio::time::interval(Duration::from_millis(400));
    let mut sent = 0;
    let mut received = 0;
    let mut buffer = vec![0; 65536];
    let mut devices = BTreeMap::new();
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            _ = retry.tick(), if sent < 3 => {
                for target in &targets {
                    udp.send_to(message.as_bytes(), target).await?;
                }
                sent += 1;
            },
            reply = udp.recv_from(&mut buffer) => {
                let (len, peer) = reply?;
                received += 1;
                if received > 1024 { break; }
                let Ok(xml) = std::str::from_utf8(&buffer[..len]) else { continue; };
                let Ok(found) = matches(xml, &id, peer) else { continue; };
                for mut device in found {
                    device.matched_streams = known.iter()
                        .filter(|(_, host)| host == &device.address)
                        .map(|(id, _)| id.clone()).collect();
                    if devices.len() < 128 {
                        devices.insert((device.id.clone(), device.address.clone()), device);
                    }
                }
            }
        }
    }
    Ok(devices.into_values().collect())
}
