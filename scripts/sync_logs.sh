#!/usr/bin/env bash
# Trae los logs del VPS al Mac (laboratorio). Ejecutar en el Mac.
#
#   VPS=ubuntu@<ip-del-vps> DEST=/Volumes/<tu-SSD>/hft-data ./scripts/sync_logs.sh
#
# Copia solo ficheros que llevan mas de 2 minutos sin modificarse, para no
# traer el log de la sesion en curso a medias... salvo con LIVE=1, que lo trae
# igualmente (el lector tolera una cola truncada).
set -euo pipefail

: "${VPS:?define VPS=usuario@host}"
DEST="${DEST:-./data}"
REMOTE_DIR="${REMOTE_DIR:-/var/lib/hft/data}"
mkdir -p "$DEST"

if [[ "${LIVE:-0}" == "1" ]]; then
  rsync -avz --partial --append-verify "$VPS:$REMOTE_DIR/" "$DEST/"
else
  ssh "$VPS" "find '$REMOTE_DIR' -name '*.hftlog' -mmin +2 -printf '%f\n'" \
    | rsync -avz --partial --files-from=- "$VPS:$REMOTE_DIR/" "$DEST/"
fi

echo
echo "Logs en $DEST:"
ls -lh "$DEST" | tail -n +2
