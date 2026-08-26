#!/usr/bin/env sh
# Put the board back to MXChip's factory firmware (bootloader + Azure IoT app).
# This undoes everything this repo does.
set -e

FW=devkit-firmware-2.0.0.bin
URL=https://github.com/microsoft/devkit-sdk/releases/download/2.0.0/$FW

[ -f "$FW" ] || curl -L -o "$FW" "$URL"

# This image is a full flash image: bootloader at 0x08000000, app at 0x0800C000.
openocd -f interface/stlink.cfg -c "transport select swd" \
        -f target/stm32f4x.cfg \
        -c "program $FW verify reset exit 0x08000000"
