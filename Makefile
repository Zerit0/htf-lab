# Atajos. `make help` los lista.
COINS ?= BTC,ETH
LOG   ?= $(shell ls -t data/*.hftlog 2>/dev/null | head -1)

.PHONY: help build test lint record replay verify stats dump synth core-test

help:
	@echo "make build       compila todo en release"
	@echo "make test        tests de todo el workspace"
	@echo "make core-test   solo los tests del nucleo (rapidos)"
	@echo "make lint        fmt + clippy"
	@echo "make record      graba Hyperliquid en data/ (COINS=BTC,ETH,HYPE)"
	@echo "make replay      ejecuta la SM sobre el ultimo log (LOG=ruta para otro)"
	@echo "make verify      comprueba que el replay es determinista"
	@echo "make stats       recuento de eventos del log"
	@echo "make dump        primeros 40 eventos crudos del log"
	@echo "make synth       genera data/synth.hftlog para probar sin red"

build:
	cargo build --release

test:
	cargo test

core-test:
	cargo test -p hft-core -p hft-replay

lint:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings

record: build
	./target/release/hft-recorder --coins $(COINS) --out-dir data

replay:
	./target/release/hft-replay run $(LOG)

verify:
	./target/release/hft-replay verify $(LOG)

stats:
	./target/release/hft-replay stats $(LOG)

dump:
	./target/release/hft-replay dump $(LOG) --limit 40

synth:
	rm -f data/synth.hftlog
	./target/release/hft-replay gen data/synth.hftlog --events 100000 --seed 1
