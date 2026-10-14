#!/usr/bin/env bash
# Install a built Java service without overwriting camera configuration.
set -euo pipefail
cd "$(dirname "$0")"
[[ $EUID == 0 ]] || { echo 'Run with sudo'; exit 1; }
jar_path="${UVMS_JAR:-uvms-java.jar}"
[[ -f "$jar_path" ]] || jar_path=target/uvms-java-0.1.0.jar
test -f "$jar_path"
command -v java >/dev/null
web_source=web
[[ -d "$web_source" ]] || web_source=../web
test -f "$web_source/index.html"
getent passwd uvms >/dev/null || useradd --system --home /opt/uvms --shell /usr/sbin/nologin uvms
install -d -m 755 /opt/uvms-java /opt/uvms-java/web /etc/uvms-java
install -d -m 700 -o uvms -g uvms /var/lib/uvms-java /var/lib/uvms-java/recordings
install -m 644 "$jar_path" /opt/uvms-java/uvms-java.jar
install -m 644 "$web_source/index.html" "$web_source/app.js" "$web_source/style.css" /opt/uvms-java/web/
if [[ ! -e /etc/uvms-java/config.json ]]; then
  install -m 640 -o root -g uvms config.example.json /etc/uvms-java/config.json
fi
if [[ ! -e /etc/uvms-java/uvms.env ]]; then
  install -m 600 /dev/null /etc/uvms-java/uvms.env
fi
install -m 644 deploy/uvms-java.service /etc/systemd/system/uvms-java.service
systemctl daemon-reload
echo 'Installed. Set REAL_CAMERA_RTSP_URL in /etc/uvms-java/uvms.env, then systemctl enable --now uvms-java.'
