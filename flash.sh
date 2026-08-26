#!/usr/bin/env sh
# Build and flash the farewell card to the AZ3166 over its on-board ST-Link/V2-1.
set -e

ELF=target/thumbv7em-none-eabihf/release/az3166-farewell
APP_ADDR=0x08000000

cargo build --release

OBJCOPY=$(command -v rust-objcopy || command -v llvm-objcopy || echo /opt/homebrew/opt/llvm/bin/llvm-objcopy)
"$OBJCOPY" -O binary "$ELF" firmware.bin
echo "firmware.bin: $(wc -c < firmware.bin | tr -d ' ') bytes -> $APP_ADDR"

# Flash the raw image at an explicit address, NOT the ELF.
#
# The ELF carries a spurious LOAD segment that maps its own file header to
# 0x08000000; `program <elf>` writes that segment, which erases flash sector 0
# and destroys the MXChip bootloader. Programming the .bin avoids this entirely.
openocd -f interface/stlink.cfg -c "transport select swd" \
        -f target/stm32f4x.cfg \
        -c "program firmware.bin verify reset exit $APP_ADDR"
