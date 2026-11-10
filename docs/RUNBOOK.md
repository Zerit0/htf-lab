# Runbook

## Alta del VPS en Tokio (una vez)

Los validadores de Hyperliquid están en AWS Tokio, así que la captura y, más adelante, la ejecución van en esa región.

En la consola de AWS:

1. Región **Asia Pacific (Tokyo) `ap-northeast-1`**.
2. EC2 → *Launch instance*:
   - AMI: **Ubuntu Server 24.04 LTS, arquitectura ARM64**.
   - Tipo: **c7g.medium** para F0–F1. Pasar a c7g.large en F2.
   - Par de claves: crear uno nuevo y guardar el `.pem` fuera del repo.
   - Red: *security group* con **solo SSH (22) desde tu IP**. Ningún puerto entrante más: el recorder solo hace conexiones salientes.
   - Disco: gp3 de 100 GB. Ajustar tras medir el volumen diario.
3. Activar un **presupuesto con alerta** en AWS Budgets (por ejemplo, 80 $/mes).

En el VPS:

```bash
sudo mkdir -p /opt/hft-lab && sudo chown "$USER" /opt/hft-lab
git clone https://github.com/Zerit0/htf-lab.git /opt/hft-lab
cd /opt/hft-lab
COINS=BTC,ETH,HYPE ./scripts/vps_bootstrap.sh
```

El script instala Rust y chrony, compila, pasa los tests, crea el usuario `hft` y arranca el servicio. No se ha probado en un VPS real: leerlo antes de ejecutarlo.

## Operación diaria

| Tarea | Comando |
|---|---|
| Ver el servicio | `sudo systemctl status hft-recorder` |
| Logs del proceso | `journalctl -u hft-recorder -f` |
| Cambiar monedas | Editar `/etc/hft/recorder.env` y `sudo systemctl restart hft-recorder` |
| Espacio en disco | `df -h /var/lib/hft` |
| Sesión en curso | `./target/release/hft-replay stats $(ls -t /var/lib/hft/data/*.hftlog \| head -1)` |
| Reconexiones | `./target/release/hft-replay dump <log> --kind conn_close` |
| Traer logs al Mac | `VPS=ubuntu@<ip> DEST=/Volumes/<SSD>/hft-data ./scripts/sync_logs.sh` |

Cada reinicio del servicio abre un fichero nuevo. Para rotar a diario basta un reinicio programado:

```bash
# /etc/cron.d/hft-rotate  (00:00 UTC)
0 0 * * * root systemctl restart hft-recorder
```

El reinicio deja un hueco de unos segundos en la captura. Para F0–F1 es aceptable; anotarlo al analizar.

## Comprobaciones de salud

- **Reloj**: `chronyc tracking`. El desfase debe estar muy por debajo de 1 ms. Si no, las medidas de latencia no valen.
- **Huecos**: muchos `CONN_CLOSE` con `stale:` indican problemas de red o del feed.
- **Formato**: `malformed` distinto de 0 en `hft-replay verify` significa que el exchange ha cambiado algo.
- **Disco**: borrar del VPS los logs ya copiados y verificados en el Mac.

## Seguridad

- Ninguna clave privada, semilla ni `.pem` en el repositorio. `.gitignore` cubre `.env`, `*.pem` y `*.key`, pero revisar siempre `git status` antes de un commit.
- F0 y F1 no usan claves de trading.
- Desde F2: wallets de API **sin permiso de retirada**, guardadas solo en el VPS en `/etc/hft/*.env` con permisos `600`. La wallet principal no toca el VPS.
- Solo depositar en los exchanges el capital de la fase en curso.
- Acceso SSH solo con clave y solo desde tu IP.

## Si algo va mal

| Síntoma | Qué mirar |
|---|---|
| El servicio se reinicia en bucle | `journalctl -u hft-recorder -n 50`. Suele ser permisos de `/var/lib/hft/data` o un fichero de log con el mismo nombre (dos arranques en el mismo segundo) |
| No llegan `WS_IN` | `dump --limit 20`: comprobar `HTTP_RESP 101` y los `WS_OUT` de suscripción. Probar `--testnet` |
| `verify` da digests distintos | Bug de determinismo en la SM. Repasar las reglas de `ARCHITECTURE.md` |
| `log corrupto` al leer | Disco o copia incompleta. Volver a sincronizar. Una cola truncada no da error, así que esto es otra cosa |
