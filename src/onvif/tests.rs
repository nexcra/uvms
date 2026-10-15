use super::*;
const OPTIONS: &str = r#"<r xmlns:tt="http://www.onvif.org/ver10/schema"><tt:ContinuousPanTiltVelocitySpace><tt:URI>urn:test:pt</tt:URI><tt:XRange><tt:Min>-2</tt:Min><tt:Max>4</tt:Max></tt:XRange><tt:YRange><tt:Min>-1</tt:Min><tt:Max>1</tt:Max></tt:YRange></tt:ContinuousPanTiltVelocitySpace><tt:ContinuousZoomVelocitySpace><tt:URI>urn:test:z</tt:URI><tt:XRange><tt:Min>-3</tt:Min><tt:Max>2</tt:Max></tt:XRange></tt:ContinuousZoomVelocitySpace><tt:PTZTimeout><tt:Min>PT0S</tt:Min><tt:Max>PT1M</tt:Max></tt:PTZTimeout></r>"#;
#[test]
fn ranges_timeouts_and_profile_tokens_are_not_assumed() {
    let options = parse_options(OPTIONS).unwrap();
    let body = movement_body("p<&", -0.5, 0.5, 0.5, 300, &options).unwrap();
    assert!(body.contains("p&lt;&amp;"));
    assert!(body.contains("x=\"-1\" y=\"0.5\""));
    assert!(body.contains("x=\"1\" space=\"urn:test:z\""));
    assert!(body.contains("PT0.300S"));
    for (v, ms) in [(f64::NAN, 300), (2., 300), (0.5, 0), (0.5, 2001)] {
        assert!(movement_body("p", v, 0., 0., ms, &options).is_err());
    }
    let none = parse_options("<r/>").unwrap();
    assert!(movement_body("p", 1., 0., 0., 300, &none).is_err());
    assert_eq!(duration_ms("PT1M2.5S"), Some(62500));
    assert_eq!(duration_ms("-PT1S"), None);
}
#[test]
fn endpoints_and_xml_reject_credentials_cross_host_and_entities() {
    for address in [
        "http://other/onvif",
        "http://user:pass@camera/onvif",
        "file:///tmp/data",
        "http://camera/onvif#x",
    ] {
        assert!(soap::endpoint(address, "camera").is_err());
    }
    assert!(soap::endpoint("https://camera:8443/onvif/device_service", "camera").is_ok());
    assert!(parse("<!DOCTYPE r [<!ENTITY x SYSTEM 'file:///etc/passwd'>]><r>&x;</r>").is_err());
}
#[test]
fn discovery_correlates_replies_and_keeps_credentials_on_the_responding_host() {
    let xml = r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery"><s:Header><a:RelatesTo>urn:test</a:RelatesTo></s:Header><s:Body><d:ProbeMatches><d:ProbeMatch><a:EndpointReference><a:Address>urn:camera</a:Address></a:EndpointReference><d:Scopes>onvif://www.onvif.org/name/Front%20Door</d:Scopes><d:XAddrs>http://127.0.0.1/onvif/device_service http://other/collect</d:XAddrs></d:ProbeMatch></d:ProbeMatches></s:Body></s:Envelope>"#;
    let peer = "127.0.0.1:3702".parse().unwrap();
    let devices = discovery::matches(xml, "urn:test", peer).unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Front Door");
    assert_eq!(devices[0].device_urls.len(), 1);
    assert!(discovery::matches(xml, "urn:other", peer).is_err());
}
#[test]
fn media_profiles_preserve_ptz_association_and_fixed_profiles() {
    let response = format!(
        r#"<r xmlns:m="{MEDIA}" xmlns:tt="{SCHEMA}"><m:Profiles token="main"><tt:Name>Main</tt:Name><tt:PTZConfiguration token="ptz1"/></m:Profiles><m:Profiles token="fixed"><tt:Name>Fixed</tt:Name></m:Profiles></r>"#
    );
    let profiles = parse_profiles(&response, MEDIA).unwrap();
    assert_eq!(profiles[0].ptz_configuration.as_deref(), Some("ptz1"));
    assert!(profiles[1].ptz_configuration.is_none());
}
