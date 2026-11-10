# Arquitectura

## Idea central

```
        BORDES (no deterministas)            NÚCLEO (determinista)
  ┌───────────────────────────────┐      ┌───────────────────────────┐
  │ red ──► session::run ──► LOG ─┼─────►│ replay::run ──► sm.update │──► Action
  │ reloj        (hilo de red)    │ tail │      (hilo de la SM)      │
  └───────────────────────────────┘      └───────────────────────────┘
                                   LOG ──► hft-replay (offline, mismo código)
```

- El **hilo de red** es el único que toca sockets y reloj. Escribe cada evento en el log.
- El **hilo de la SM** sigue el fichero y llama a `sm.update(evento)`.
- En **replay** no hay hilo de red: se lee un fichero cerrado. El código de la SM es el mismo, byte a byte.

El fichero como canal entre hilos es un apaño deliberado (paso 2 del hilo). En el paso 9 se envuelve en una interfaz de cola y más adelante se sustituye por una cola en memoria compartida al estilo Chronicle-Queue. La latencia que añade todavía no importa.

## Formato del log (`.hftlog`)

Little-endian. Un fichero por sesión del recorder.

| Campo | Tamaño | Descripción |
|---|---|---|
| Cabecera de fichero | 8 bytes | `HFTLOG01` |
| `len` | u32 | Bytes que siguen a este campo: 20 + payload + 4 |
| `seq` | u64 | Secuencia, desde 0, sin huecos |
| `ts_ns` | u64 | Reloj local en ns Unix al ver o emitir el evento |
| `kind` | u8 | Tipo de evento (tabla siguiente) |
| `venue` | u8 | 0 ninguno, 1 Hyperliquid, 2 Lighter |
| `conn` | u16 | Id de conexión dentro de la sesión |
| `payload` | variable | Bytes crudos (JSON del exchange, URL, motivo...) |
| `checksum` | u32 | FNV-1a 32 de `seq` a `payload` |

Propiedades:

- **Una sola llamada `write` por registro**: un lector en tail solo puede ver un registro incompleto en la cola, y entonces espera.
- **Tolerante a cortes**: tras un apagón queda, como mucho, un registro truncado al final, que se ignora.
- **Detecta corrupción**: checksum, secuencia sin huecos y longitud acotada (16 MiB).
- **Payload crudo**: se guarda lo que envió el exchange, sin interpretar. Si mañana cambia el decoder, los logs antiguos se pueden volver a procesar.

| `kind` | Nombre | Payload |
|---|---|---|
| 1 | `SESSION_START` | Versión y argumentos del recorder |
| 2 | `CONN_OPEN` | URL |
| 3 | `CONN_CLOSE` | Motivo (`closed:`, `error:`, `stale:`, `connect_error:`, `shutdown`) |
| 4 | `HTTP_REQ` | Petición saliente |
| 5 | `HTTP_RESP` | Status y cabeceras |
| 6 | `WS_IN` | Mensaje entrante crudo |
| 7 | `WS_OUT` | Mensaje saliente, registrado antes de enviarse |
| 8 | `TIMER` | Vacío. Tick de 1 s |
| 9 | `ACTION` | Reservado (F2) |
| 10 | `NOTE` | Texto libre |
| 11 | `WS_CTL` | `ping_in` / `pong_in` |

Volumen orientativo, medido con el generador sintético: unos 400 bytes por actualización de libro de 5 niveles. El feed real trae más niveles por mensaje, así que conviene medirlo el primer día con `hft-replay stats` antes de dimensionar el disco.

## Reglas de determinismo

La state machine debe dar la misma salida ante la misma secuencia de eventos, en cualquier máquina. Para eso:

| Regla | Motivo |
|---|---|
| El tiempo sale de `ev.ts_ns`, nunca de `SystemTime::now()` | En replay el reloj real es otro |
| `BTreeMap`, nunca `HashMap` | El orden de iteración de `HashMap` cambia en cada ejecución |
| Precios y tamaños en `Fixed` (i64 escalado a 1e8), nunca `f64` | Los redondeos en coma flotante dependen del orden de operaciones |
| Nada de aleatoriedad sin semilla explícita guardada en el log | Paso 14: misma semilla + captura + estrategia = mismo resultado |
| La SM no hace E/S: devuelve `Action` y texto | La E/S es de los bordes |
| Los ids de orden (`cloid`) los genera la SM a partir de su estado | Para que el replay reproduzca las mismas órdenes |

Cómo se vigila:

- `hft-replay verify <log>` ejecuta dos veces y compara el digest (FNV-1a 64 de toda la salida).
- El test `replay::tests::live_follow_matches_replay` escribe un log desde un hilo mientras otro lo sigue, y exige que la salida en vivo sea idéntica a la del replay posterior.
- La CI compara el digest en Linux ARM y macOS ARM.

## Bucle de sesión (`session::run`)

Por cada conexión:

1. `CONN_OPEN`, `HTTP_REQ`, conectar, `HTTP_RESP`.
2. Por cada suscripción: `WS_OUT` y enviar.
3. Leer con timeout (100 ms por defecto). Texto y binario van a `WS_IN`.
4. Tras **cada** lectura se evalúan los temporizadores: `TIMER` cada 1 s, heartbeat cada 20 s, feed muerto a los 45 s sin entrantes, `fsync` cada 5 s.
5. Al cerrar: `CONN_CLOSE` con el motivo, backoff exponencial (0,5 s → 30 s, vuelve al mínimo al recibir datos) y reconexión con `conn + 1`.

Al cerrarse una conexión, la SM **descarta los libros de ese venue**: ninguna estrategia debe cotizar contra un libro caducado.

## Limitaciones conocidas de F0

| Limitación | Se resuelve en |
|---|---|
| `tungstenite::connect` bloquea sin timeout en DNS, TCP y TLS | Paso 7: reactor sobre mio |
| Latencia de hasta 100 ms en temporizadores cuando no hay tráfico | Paso 7 |
| Sin timestamps del kernel | Paso 8 (solo Linux) |
| El canal entre hilos es un fichero con sondeo cada 0,5 ms | Paso 9 |
| Parser JSON genérico, no optimizado | Cuando el simulador diga que importa |
| Solo Hyperliquid | F3: Lighter |
| El recorder termina por señal, sin cierre ordenado | F2, junto con el kill switch |
