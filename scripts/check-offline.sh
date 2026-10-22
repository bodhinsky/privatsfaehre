#!/usr/bin/env bash
# Offline guarantees, enforced in CI:
#   1. no HTTP / TLS / WebSocket / DNS client crate anywhere in the dependency
#      tree (all features, all targets, normal + build + dev dependencies);
#   2. no workspace crate depends on the UI crate (`app`) or a UI toolkit;
#   3. no socket APIs in our own source code.
set -euo pipefail
cd "$(dirname "$0")/.."

BANNED=(
  reqwest ureq hyper hyper-util hyper-rustls hyper-tls h2 h3 h3-quinn quinn quinn-proto
  isahc surf attohttpc curl curl-sys minreq ehttp awc http-client http-req
  tungstenite tokio-tungstenite async-tungstenite websocket
  native-tls rustls tokio-rustls tokio-native-tls openssl openssl-sys
  trust-dns-resolver hickory-resolver hickory-proto
  sentry opentelemetry-otlp self_update
)
UI_CRATES=(app eframe egui egui-winit winit tauri wry tao tray-icon)

fail=0

deps=$(cargo tree --workspace --all-features --target all -e normal,build,dev --prefix none --format '{p}' \
  | awk '{print $1}' | sort -u)

for c in "${BANNED[@]}"; do
  if grep -qx -- "$c" <<<"$deps"; then
    echo "::error::banned network crate '$c' is in the dependency tree:"
    cargo tree --workspace --all-features --target all -e normal,build,dev -i "$c" || true
    fail=1
  fi
done

for member in detect vault dataset clip ingest cli; do
  member_deps=$(cargo tree -p "$member" --all-features --target all -e normal,build,dev --prefix none --format '{p}' \
    | awk '{print $1}' | sort -u)
  for ui in "${UI_CRATES[@]}"; do
    if grep -qx -- "$ui" <<<"$member_deps"; then
      echo "::error::crate '$member' depends on UI crate '$ui'"
      fail=1
    fi
  done
done

if grep -rnE 'TcpStream|TcpListener|UdpSocket|ToSocketAddrs|UnixStream|UnixListener' crates/ --include='*.rs'; then
  echo "::error::socket API used in workspace source"
  fail=1
fi

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "offline check passed: $(wc -l <<<"$deps") crates, no network clients, no UI dependencies"
