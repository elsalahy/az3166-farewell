# az3166-farewell

A farewell card that runs on hardware.

This is firmware for the [MXChip AZ3166 IoT DevKit](https://en.mxchip.com/az3166) that does
exactly one thing: it cycles a handful of thank-you messages across the board's little 128x64
OLED, forever, on a loop. No Wi-Fi, no cloud, no sensors, no telemetry. Plug it into any USB
port — a laptop, a phone charger, a power bank — and it says its piece.

A progress bar creeps across the panel's yellow stripe as it goes, so you can see the next
card coming and where you are in the loop.

Written in Rust, `#![no_std]`, no RTOS. About 14 KB of firmware.

## Editing the messages

Everything you'd want to change lives in one array at the top of [`src/main.rs`](src/main.rs):

```rust
static CARDS: &[Card] = &[
    Card::Text("THANK YOU", &["Joel"]),
    Card::Text("", &["for everything", "you built with us", "at Q-Bird"]),
    // ...
    Card::Ferris,
];
```

Each card is one screen, shown for `CARD_DWELL_MS` before the next one. Width budget:
**14 characters** for a headline (9x15 bold), **21 characters** for a body line (6x10).
Pass `""` as the headline to leave it off. Text is centred horizontally and the block is
centred vertically, so short cards look fine.

Change the strings, run `./flash.sh`, done — about 20 seconds.

### Ferris

The last card is Ferris, the Rust mascot, hand-drawn as ASCII art in the same file:

```rust
static FERRIS: &[&str] = &[
    ".................##############.................",
    "...............##################...............",
    // ...
];
```

`#` is a lit pixel. `expand_ferris()` blows it up 2x into a 1bpp MSB-first bitmap at build
time — 48x17 becomes 96x34, which is exactly 12 bytes per row, so there's no row padding to
get wrong. Every index is bounded by `FERRIS_W` / `FERRIS_H`, so if you edit the art and
miscount a row you get a clipped crab rather than a panicking board.

Redraw him however you like; keep the source art within **64 x 24** so the 2x version still
fits the blue part of the panel.

## Flashing it

The board has an ST-Link/V2-1 built in, so you don't need any extra hardware — just the
USB cable.

```sh
brew install openocd llvm          # one time
rustup target add thumbv7em-none-eabihf
./flash.sh
```

`flash.sh` builds, converts the ELF to a raw image, and programs it with OpenOCD, verifying
the write. The board resets into the card loop immediately.

If you just want to run it without installing a Rust toolchain, `firmware.bin` in this repo is
the prebuilt image:

```sh
openocd -f interface/stlink.cfg -c "transport select swd" \
        -f target/stm32f4x.cfg \
        -c "program firmware.bin verify reset exit 0x08000000"
```

## Putting the board back to normal

```sh
./restore-factory.sh
```

Downloads MXChip's official `devkit-firmware-2.0.0.bin` and flashes it. That image is a full
flash image — bootloader at `0x08000000`, Azure IoT demo app at `0x0800C000` — so the board
comes back exactly as it shipped.

## Hardware notes

Collected the hard way; recorded here so nobody has to dig for them again.

| | |
|---|---|
| MCU | STM32F412RG, Cortex-M4F, 1 MB flash / 256 KB RAM |
| Rust target | `thumbv7em-none-eabihf` |
| OLED | SSD1306 128x64 |
| OLED bus | **I2C1**, **PB8 = SCL**, **PB9 = SDA**, address **0x3C** |
| OLED reset | **PA8** — must be driven high before the panel will talk (see below) |
| OLED orientation | panel is mounted with segment-remap + inverted COM scan → `DisplayRotation::Rotate180` |
| Debugger | on-board **ST-Link/V2-1**, USB `0x0483:0x374b` (*not* CMSIS-DAP, despite the mbed-style `DETAILS.TXT` on the mass-storage drive) |
| Serial | `/dev/cu.usbmodem*` @ 115200 (USART6, PA11/PA12) — unused here |

The pinout came from [Zephyr's upstream board devicetree](https://github.com/zephyrproject-rtos/zephyr/blob/main/boards/mxchip/az3166_iotdevkit/az3166_iotdevkit.dts),
which is the most reliable description of this board I found.

### Why the firmware links at `0x08000000`

Stock, the flash is split: MXChip's bootloader at `0x08000000`, and the user application at
`0x0800C000` (you can see that offset in devkit-sdk's own OpenOCD upload recipe in
`platform.txt`).

Linking there and letting the bootloader chain-load us *doesn't work* — the bootloader won't
hand off to a non-MXChip image. The board boots, stays in the bootloader spinning in RAM,
prints nothing on the serial console, and never reaches the app. So this firmware owns flash
from `0x08000000` and boots directly. `restore-factory.sh` puts MXChip's layout back.

### The panel is two-tone, so keep text out of the seam

The OLED module isn't a uniform white 128x64 — it's one of the common two-tone panels: 16
rows of yellow, 48 rows of blue, with a hard colour break where they meet and no way to
change it in software. It's a property of the glass.

After `Rotate180` the yellow band is the **bottom 16 rows** in drawing coordinates, so
anything crossing `y = 48` comes out half yellow and half blue. `top_of_block()` therefore
centres each card within `BLUE_ROWS` (0..47) rather than the full 64, which keeps every line
on one side of the seam.

You can't switch the colour off — it's a stripe on the glass. But an OLED pixel that is off
emits nothing, so an unlit yellow pixel is simply black: the band is invisible until you
light something in it. That's the whole trick to this firmware never showing yellow. Every
card draws within rows 0..47, including the frame, which is `128 x 48` rather than
`128 x 64` for exactly this reason.

Flip it around and the band becomes a second colour rather than a hazard: anything you draw
at `y >= 48` comes out yellow. That's what the progress bar uses it for — `draw_progress()`
owns rows 48..63 and nothing else touches them, so the bar is the only yellow on the panel
and reads as an accent instead of a defect.

That split also makes the animation cheap. ssd1306 tracks a dirty rectangle and flushes only
what changed, so a progress step sends two pages (~256 bytes, ~6 ms over I2C) rather than the
full 1 KB framebuffer. Only the first flush after a card change is a full one, because
`clear()` dirties everything. Forty full flushes per card would have spent a third of the
dwell time pushing pixels.

If you add cards, keep the block under 48 px tall: a headline plus two body lines (42 px) is
the practical maximum.

### The OLED will not answer until you release its reset line

The panel's `RES#` is wired to **PA8**. Out of a cold reset PA8 is a floating input, so the
SSD1306 powers up but stays held in reset and never ACKs its address — you get a
`DisplayError` out of `init()` and a blank screen, with nothing obviously wrong on the bus.
MXChip's bootloader releases the line for you, which is why this only bites when you boot
your own image directly.

Pulse it before touching I2C:

```rust
let mut oled_reset = gpioa.pa8.into_push_pull_output();
oled_reset.set_low();
sleep_ms(10);
oled_reset.set_high();
sleep_ms(100);
```

The symptom, if you skip it: `mdw 0x40005400 10` shows I2C1 correctly configured with
`DR = 0x78` (= `0x3C << 1`, so the address went out) and `SR2 = 0x3` (MSL + BUSY) — a master
that started a transfer nobody answered. GPIOB reads back perfectly (`AFRH = 0x44` = AF4,
open-drain, both lines idle-high), which sends you hunting for a bus fault that isn't there.

This line is easy to miss: it is in
[Zephyr's devicetree](https://github.com/zephyrproject-rtos/zephyr/blob/main/boards/mxchip/az3166_iotdevkit/az3166_iotdevkit.dts)
as `reset-gpios = <&gpioa 8 GPIO_ACTIVE_HIGH>`, but most AZ3166 pinout diagrams don't mention it.

### The trap that costs you a bootloader

**Program the `.bin` at an explicit address. Do not `program` the ELF.**

The linker emits a spurious `LOAD` segment that maps the ELF's own file header to
`0x08000000`:

```
LOAD  0x000000 0x08000000 0x08000000 FileSiz 0x00154  R
LOAD  0x00c000 0x0800c000 0x0800c000 FileSiz 0x001c4  R    <- .vector_table
```

`openocd -c "program firmware.elf ..."` writes segments, so it dutifully flashes that header
to `0x08000000` — erasing flash sector 0 and taking the bootloader with it. The tell is this
line scrolling past:

```
Info : Flash write discontinued at 0x08000154, next section at 0x0800c000
```

and afterwards `mdw 0x08000000` reads back `464c457f` — `\x7fELF`. Recoverable with
`restore-factory.sh`, but easier to just not do it.

## Layout

```
src/main.rs         the card deck and ~90 lines of plumbing
memory.x            flash/RAM layout
build.rs            hands memory.x to the linker
flash.sh            build + flash
restore-factory.sh  put MXChip's firmware back
firmware.bin        prebuilt image, flash it at 0x08000000
```

## License

MIT.
