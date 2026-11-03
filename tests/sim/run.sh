#!/bin/sh
# basil-sim-only tests: programs that need a stimulus file. Run after `cargo build --release` here and in ../Basic_Compiler.
# Usage: tests/sim/run.sh
set -e
cd "$(dirname "$0")/../.."
BASIL=${BASIL:-$(pwd)/../Basic_Compiler/target/release/basil}
w=$(mktemp -d)
trap 'rm -rf "$w"' EXIT
want="SRX_B0 = 72
SRX_B1 = 105
SRX_B2 = 33
SRX_B3 = 63
SRX_FERR = %s
tx1: Hi!"
for t in "serial_rx 18F27Q10 4" "serial_rx_q41 18F16Q41 8"; do
    set -- $t
    cp tests/sim/$1.bas "$w/"
    "$BASIL" --keep-vars "$w/$1.bas" >/dev/null
    out=$(./target/release/basil-sim "$w/$1.hex" --device $2 --stim tests/sim/serial_rx.stim.toml \
        --dump SRX_B0,SRX_B1,SRX_B2,SRX_B3,SRX_FERR 2>/dev/null)
    for l in $(printf "$want\n" "$3" | tr ' ' '_'); do
        echo "$out" | tr ' ' '_' | grep -q "^$l" || { echo "FAIL $1: missing '$l'"; echo "$out"; exit 1; }
    done
    echo "PASS $1"
done
cp tests/sim/serial_tx_q41.bas "$w/"
"$BASIL" "$w/serial_tx_q41.bas" >/dev/null
out=$(./target/release/basil-sim "$w/serial_tx_q41.hex" --device 18F16Q41 2>/dev/null)
echo "$out" | grep -q "^tx1: AB$" || { echo "FAIL serial_tx_q41: want 'tx1: AB'"; echo "$out"; exit 1; }
echo "PASS serial_tx_q41"
cp tests/sim/adc_q41.bas "$w/"
"$BASIL" --keep-vars "$w/adc_q41.bas" >/dev/null
out=$(./target/release/basil-sim "$w/adc_q41.hex" --device 18F16Q41 --stim tests/sim/adc_q41.stim.toml --dump ADQ_A0:2,ADQ_A1:2,ADQ_A2:2 2>/dev/null)
for l in "ADQ_A0 = 2748" "ADQ_A1 = 0 " "ADQ_A2 = 291"; do
    echo "$out" | grep -q "^$l" || { echo "FAIL adc_q41: missing '$l'"; echo "$out"; exit 1; }
done
echo "PASS adc_q41"
