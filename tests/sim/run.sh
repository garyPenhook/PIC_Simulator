#!/bin/sh
# basil-sim-only tests: programs that need a stimulus file. Run after `cargo build --release`.
# Usage: [BASIL=path/to/basil] tests/sim/run.sh   (the compiler lives in the Basic_Compiler repo; default ../Basic_Compiler/target/release/basil)
set -e
cd "$(dirname "$0")/../.."
BASIL=$(realpath "${BASIL:-../Basic_Compiler/target/release/basil}")
[ -x "$BASIL" ] || { echo "basil compiler not found at $BASIL; build Basic_Compiler or set BASIL"; exit 1; }
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
cp tests/sim/adc_p16.bas "$w/"
"$BASIL" --keep-vars "$w/adc_p16.bas" >/dev/null
out=$(./target/release/basil-sim "$w/adc_p16.hex" --device 16F17146 --stim tests/sim/adc_q41.stim.toml --dump ADP_A0:2,ADP_A1:2,ADP_A2:2 2>/dev/null)
for l in "ADP_A0 = 2748" "ADP_A1 = 0 " "ADP_A2 = 291"; do
    echo "$out" | grep -q "^$l" || { echo "FAIL adc_p16: missing '$l'"; echo "$out"; exit 1; }
done
echo "PASS adc_p16"
cp tests/sim/spi_q41.bas "$w/"
"$BASIL" --keep-vars "$w/spi_q41.bas" >/dev/null
out=$(./target/release/basil-sim "$w/spi_q41.hex" --device 18F16Q41 --stim tests/sim/spi_q41.stim.toml --dump SQ1_SQ_INIT,SQ1_SQ_R0,SQ1_SQ_R1,SQ1_SQ_R2,SQ1_SQ_ST 2>/dev/null)
for l in "SQ1_SQ_INIT = 1 " "SQ1_SQ_R0 = 239" "SQ1_SQ_R1 = 64" "SQ1_SQ_R2 = 24" "SQ1_SQ_ST = 0 " "spi1: 9F 00 FF"; do
    echo "$out" | grep -q "^$l" || { echo "FAIL spi_q41: missing '$l'"; echo "$out"; exit 1; }
done
echo "PASS spi_q41"
cp tests/sim/spi_q10.bas "$w/"
"$BASIL" --keep-vars "$w/spi_q10.bas" >/dev/null
out=$(./target/release/basil-sim "$w/spi_q10.hex" --device 18F27Q10 --stim tests/sim/spi_q10.stim.toml --dump SQ10_SQ_INIT,SQ10_SQ_R0,SQ10_SQ_R1,SQ10_SQ_R2,SQ10_SQ_ST 2>/dev/null)
for l in "SQ10_SQ_INIT = 1 " "SQ10_SQ_R0 = 239" "SQ10_SQ_R1 = 64" "SQ10_SQ_R2 = 24" "SQ10_SQ_ST = 0 " "spi1: 9F 00 FF"; do
    echo "$out" | grep -q "^$l" || { echo "FAIL spi_q10: missing '$l'"; echo "$out"; exit 1; }
done
echo "PASS spi_q10"
cp tests/sim/i2c_q10.bas "$w/"
"$BASIL" --keep-vars "$w/i2c_q10.bas" >/dev/null
out=$(./target/release/basil-sim "$w/i2c_q10.hex" --device 18F27Q10 --stim tests/sim/i2c_q10.stim.toml --dump IQ10_IQ_INIT,IQ10_IQ_S0,IQ10_IQ_S1,IQ10_IQ_S2,IQ10_IQ_S3,IQ10_IQ_R0,IQ10_IQ_R1,IQ10_IQ_NAK 2>/dev/null)
for l in "IQ10_IQ_INIT = 1 " "IQ10_IQ_S1 = 0 " "IQ10_IQ_S2 = 0 " "IQ10_IQ_R0 = 202" "IQ10_IQ_R1 = 254" "IQ10_IQ_NAK = 3 " "i2c1: S 50W 10 Sr 50R CA FE! P S 5AW! P"; do
    echo "$out" | grep -q "^$l" || { echo "FAIL i2c_q10: missing '$l'"; echo "$out"; exit 1; }
done
echo "PASS i2c_q10"
cp tests/sim/blink_q10.bas "$w/"
"$BASIL" "$w/blink_q10.bas" >/dev/null
out=$(./target/release/basil-sim "$w/blink_q10.hex" --device 18F27Q10 --stim tests/sim/blink.stim.toml --max-cycles 40000000 2>/dev/null) || true  # stopping at --max-cycles exits 1
for l in "led heartbeat (RB0, active high): ON, on 23999990 of 40000000 cycles, 5 changes" "led error (RB1, active low): off, on 16000010 of 40000000 cycles, 6 changes"; do
    echo "$out" | grep -qF "$l" || { echo "FAIL blink_q10: missing '$l'"; echo "$out"; exit 1; }
done
echo "PASS blink_q10"
