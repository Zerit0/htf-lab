#!/usr/bin/env bash
# Prepara un VPS Ubuntu 24.04 ARM64 recien creado (AWS ap-northeast-1) para F0.
# Ejecutar UNA vez, como usuario con sudo, desde la raiz del repo ya clonado
# en /opt/hft-lab:
#
#   sudo mkdir -p /opt/hft-lab && sudo chown "$USER" /opt/hft-lab
#   git clone https://github.com/Zerit0/htf-lab.git /opt/hft-lab
#   cd /opt/hft-lab && ./scripts/vps_bootstrap.sh
#
# No se ha ejecutado en un VPS real: leelo antes de lanzarlo.
set -euo pipefail

COINS="${COINS:-BTC,ETH}"
REPO_DIR="/opt/hft-lab"
DATA_DIR="/var/lib/hft/data"

if [[ "$(pwd)" != "$REPO_DIR" ]]; then
  echo "ejecuta este script desde $REPO_DIR" >&2
  exit 1
fi

echo "==> paquetes del sistema"
sudo apt-get update -y
# chrony: los timestamps locales son la base del estudio de latencia.
sudo apt-get install -y build-essential pkg-config chrony rsync curl git

echo "==> sincronizacion horaria (Amazon Time Sync)"
if ! grep -q "169.254.169.123" /etc/chrony/chrony.conf; then
  echo "server 169.254.169.123 prefer iburst minpoll 4 maxpoll 4" | sudo tee -a /etc/chrony/chrony.conf >/dev/null
  sudo systemctl restart chrony
fi
chronyc tracking | sed -n '1,6p' || true

echo "==> rust"
if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
  # shellcheck disable=SC1091
  source "$HOME/.cargo/env"
fi
rustc --version

echo "==> compilacion y tests"
cargo test
cargo build --release

echo "==> usuario y carpetas"
id -u hft >/dev/null 2>&1 || sudo useradd --system --home /var/lib/hft --shell /usr/sbin/nologin hft
sudo mkdir -p "$DATA_DIR" /etc/hft
sudo chown -R hft:hft /var/lib/hft
echo "COINS=$COINS" | sudo tee /etc/hft/recorder.env >/dev/null
# el usuario hft debe poder leer el binario
sudo chmod -R o+rX "$REPO_DIR"

echo "==> servicio systemd"
sudo cp deploy/hft-recorder.service /etc/systemd/system/hft-recorder.service
sudo systemctl daemon-reload
sudo systemctl enable --now hft-recorder
sleep 3
sudo systemctl --no-pager status hft-recorder | sed -n '1,12p'

echo
echo "Listo. Comprobaciones:"
echo "  journalctl -u hft-recorder -f"
echo "  ls -lh $DATA_DIR"
echo "  ./target/release/hft-replay stats \$(ls -t $DATA_DIR/*.hftlog | head -1)"
