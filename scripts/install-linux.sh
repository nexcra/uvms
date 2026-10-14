#!/usr/bin/env bash
# Install the already built Linux binary. Keeps existing config/credentials.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $EUID == 0 ]] || { echo "Run with sudo"; exit 1; }
binary="${UVMS_BIN:-target/release/uvms-gateway}"
test -f "$binary"
getent passwd uvms >/dev/null || useradd --system --home /opt/uvms --shell /usr/sbin/nologin uvms
install -d -m 755 /opt/uvms /opt/uvms/web /etc/uvms
install -d -m 700 -o uvms -g uvms /var/lib/uvms /var/lib/uvms/recordings
install -m 755 "$binary" /opt/uvms/uvms-gateway
install -m 644 web/index.html web/app.js web/style.css /opt/uvms/web/
if [[ -d web/vendor ]]; then
  install -d -m 755 /opt/uvms/web/vendor
  install -m 644 web/vendor/hls.min.js web/vendor/hls.LICENSE /opt/uvms/web/vendor/
fi
if [[ ! -e /etc/uvms/config.toml ]]; then install -m 640 -o root -g uvms config.example.toml /etc/uvms/config.toml; fi
# Remove obsolete gateway authentication settings during an upgrade.
sed -i -E '/^[[:space:]]*(local_access|token_env)[[:space:]]*=/d' /etc/uvms/config.toml
if [[ -f /etc/uvms/uvms.env ]]; then
  sed -i -E '/^[[:space:]]*UVMS_TOKEN=/d' /etc/uvms/uvms.env
  chmod 600 /etc/uvms/uvms.env
fi
install -m 644 deploy/uvms.service /etc/systemd/system/uvms.service
systemctl daemon-reload
echo "Installed. Set /etc/uvms/uvms.env (mode600) and config.toml, then systemctl enable --now uvms."
