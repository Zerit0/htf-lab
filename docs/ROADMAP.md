# Roadmap

Ritmo previsto: 8–10 h por semana. Cada fase acaba en un criterio medible. Si no se cumple, se para: el coste hundido no cuenta.

## Tesis que hay que validar

Estrategia objetivo (paso 12 del hilo): orden **maker en Lighter** al precio al que se puede cubrir como **taker en Hyperliquid**, más un margen.

Coste por fill, con las tarifas publicadas en noviembre de 2026 (comprobar antes de F3, cambian):

| Pata | Comisión |
|---|---|
| Hyperliquid taker, tier base | 4,5 bps |
| Lighter Premium maker, sin LIT en staking | 0,4 bps |
| **Total** | **~4,9 bps** |

La cuenta Standard de Lighter es gratuita pero añade 200–300 ms de latencia, así que queda descartada para esto. Solo hay beneficio cuando alguien cruza nuestra orden a más de ~5 bps del precio de cobertura. La pregunta de F1 es **con qué frecuencia ocurre y en qué instrumentos**.

## Fases

| Fase | Semanas | Pasos del hilo | Entregable | Capital en riesgo |
|---|---|---|---|---|
| **F0 · Captura** | 1 | 1, 2, 3 | Recorder 24 h en el VPS de Tokio, replay determinista | 0 $ |
| **F1 · Estudio** | 2–4 | — | Segundo recorder (Lighter) y estudio offline de oportunidades | 0 $ |
| **Go/No-Go 1** | 4 | | | |
| **F2 · Órdenes** | 5–7 | 4–9 | Place/cancel en vivo, reactor, kill switch | 300–500 $ |
| **F3 · Cross-venue** | 8–11 | 10–13 | Market making ingenuo y maker-taker con cobertura | 2.000–3.000 $ |
| **Go/No-Go 2** | 11 | | | |
| **F4 · Escalar** | 12+ | 14–15 | Simulador, señales, mejores tiers | 10.000–20.000 $ |

## F0 · Checklist

- [x] `cargo test` y `cargo build --release` pasan en la CI (Linux ARM y macOS ARM), mismo digest en ambas.
- [ ] `cargo test` y `cargo build --release` pasan en el Mac.
- [ ] Grabación de prueba desde el Mac: `malformed 0`, `DETERMINISTA OK`.
- [ ] Anotar el `lag feed (ms)` desde Madrid.
- [ ] VPS en `ap-northeast-1` con `scripts/vps_bootstrap.sh` (ver `RUNBOOK.md`).
- [ ] Anotar el `lag feed (ms)` desde Tokio y compararlo.
- [ ] 24 h de grabación sin huecos: `hft-replay dump <log> --kind conn_close` muestra pocas reconexiones y todas explicadas.
- [ ] Mismo digest para el mismo fichero en el Mac y en el VPS.
- [ ] Medir GB por día y por moneda, y ajustar el disco.

**Criterio de salida:** un log de 24 h que se reproduce con el mismo digest en las dos máquinas.

## F1 · Estudio de oportunidades

Trabajo:

1. Decoder y sesión de Lighter (`venue/lighter.rs`). Verificar su API en la documentación oficial antes de escribir nada.
2. Grabar los dos venues a la vez, 5–7 días, en 8–12 instrumentos de liquidez media.
3. Herramienta `hft-study` que, sobre los logs, calcule para cada instrumento y cada instante:
   - precio de cobertura taker en Hyperliquid para un tamaño dado (`Book::taker_fill` ya existe);
   - a qué precio habría que cotizar en Lighter para ganar X bps netos;
   - cuántas veces un trade en Lighter habría cruzado esa cotización.
4. Informe por instrumento: oportunidades por día, edge neto medio, tamaño disponible.

**Go/No-Go 1.** Seguir solo si hay al menos 2–3 instrumentos con:

- edge neto medio > 1 bp después de los ~4,9 bps de coste, y
- varias decenas de oportunidades al día, y
- tamaño suficiente para que el beneficio diario esperado supere el coste del VPS con holgura.

Sesgo conocido del estudio: es optimista. No modela nuestra posición en la cola ni que los competidores reaccionan a nuestra orden. Un resultado justo en el límite es un No-Go.

## F2 · Órdenes (pasos 4–9)

- `sm.update` devuelve `Place` / `Cancel`; el hilo de red registra la acción **antes** de ejecutarla y la ignora en replay.
- Firma de órdenes de Hyperliquid con una wallet de API **sin permiso de retirada**.
- Primer test en vivo: colocar y cancelar una orden lejos del mercado, con saldo mínimo, y exigir que el replay reproduzca exactamente las mismas acciones.
- Reactor no bloqueante (mio: kqueue en Mac, epoll en Linux), temporizador real, timestamps del kernel en Linux.
- Interfaz de cola alrededor del fichero.
- **Salvaguardas obligatorias antes del primer dólar:** inventario máximo, pérdida diaria máxima, kill switch manual y cancelación de todo al desconectar.

## F3 · Cross-venue (pasos 10–13)

- Market making ingenuo con sesgo por inventario.
- Dividir la SM en tres: estado, estrategia, reconciliador (paso 11).
- Maker en Lighter + cobertura IOC en Hyperliquid, con spread pequeño para forzar fills y encontrar bugs.
- Revisión línea a línea de logs: comisiones, fills buenos y malos, cancelaciones innecesarias.
- Funcionamiento 24 h con desconexiones.

**Go/No-Go 2.** Tras dos semanas en vivo con tamaño pequeño:

- P&L neto de comisiones ≥ 0, y
- decenas de fills por hora, y
- el replay reproduce las acciones de cada sesión.

Stop total si la pérdida acumulada llega a 500 $.

## F4 · Escalar (pasos 14–15)

- Simulador con aleatoriedad sembrada, reconciliado con el trading real.
- Proceso de investigación de señales sobre capturas de 24 h.
- Subir de tier de comisiones y revisar si compensa el staking.

## Costes

| Concepto | Estimación |
|---|---|
| VPS en Tokio (F0–F1: c7g.medium; F2+: c7g.large) | ~45–110 $/mes |
| Gas y puentes | 30–50 $ |
| Pérdida esperada por comisiones y bugs en F2–F3 | 200–500 $ |
| Capital de trading hasta F3 (recuperable) | 2.500–3.500 $ |

## Antes de F3

- Hablar con el gestor: miles de operaciones al año tributan en el IRPF. Los logs son el registro.
- Volver a comprobar tarifas y condiciones de cuenta de los dos exchanges.
