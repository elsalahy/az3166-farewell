//! A farewell card that runs forever on the OLED of an MXChip AZ3166 IoT DevKit.
//!
//! Hardware: STM32F412RG + SSD1306 128x64 on I2C1 (PB8 = SCL, PB9 = SDA, addr 0x3C).
//! Nothing else on the board is touched - no Wi-Fi, no sensors, no audio.

#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

use embedded_graphics::{
    draw_target::DrawTarget,
    image::{Image, ImageRaw},
    mono_font::{
        ascii::{FONT_6X10, FONT_9X15_BOLD},
        MonoFont, MonoTextStyle,
    },
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use ssd1306::{prelude::*, I2CDisplayInterface, Ssd1306};
use stm32f4xx_hal::{pac, prelude::*};

// ---------------------------------------------------------------------------
// THE ONLY PART YOU NEED TO EDIT
//
// Each card is one screen. It shows for CARD_DWELL_MS, then the next one, forever.
// ---------------------------------------------------------------------------

enum Card {
    /// A headline plus body lines. Pass "" as the headline to leave it off.
    /// Width budget: headline <= 14 characters, body lines <= 21 characters.
    Text(&'static str, &'static [&'static str]),
    /// Ferris, the Rust mascot, drawn from the ASCII art further down.
    Ferris,
}

static CARDS: &[Card] = &[
    Card::Text("THANK YOU", &["Joel"]),
    Card::Text("", &["for everything", "you built with us", "at Q-Bird"]),
    Card::Text("GOOD LUCK", &["on whatever", "you build next"]),
    Card::Text("", &["- Ahmed", "& the Q-Bird team"]),
    Card::Text("", &["(this card runs on", "128x64 px of Rust)"]),
    Card::Ferris,
];

/// How long each card stays on screen.
const CARD_DWELL_MS: u32 = 2_500;

/// Ferris, drawn by hand. '#' is a lit pixel, everything else is dark.
///
/// Eyes on stalks above the shell, claws held low and wide, six legs underneath.
/// Every row must be exactly FERRIS_W characters and there must be FERRIS_H of
/// them - anything longer is clipped rather than panicking, so a miscount shows
/// up as a chopped crab rather than a dead board. Drawn at 2x, so the sprite
/// must stay within 64 x 24 to fit the blue part of the panel.
static FERRIS: &[&str] = &[
    "......................######........######......................",
    "......................##..##........##..##......................",
    "......................##..##........##..##......................",
    "......................######........######......................",
    "..................############################..................",
    "...............##################################...............",
    ".............######################################.............",
    "............########################################............",
    "...........##########################################...........",
    "..........############################################..........",
    "..........############################################..........",
    "..........############################################..........",
    "..........################..########..################..........",
    ".......#####################........#####################.......",
    "....########################################################....",
    "#########..##########################################..#########",
    "########....########################################....########",
    "....#####.....####################################.....#####....",
    "#########........##############################........#########",
    "########.............######################.............########",
    "....................###...###......###...###....................",
    "....................##.....##......##.....##....................",
];

const FERRIS_W: usize = 64;
const FERRIS_H: usize = 22;

// ---------------------------------------------------------------------------
// Everything below is plumbing.
// ---------------------------------------------------------------------------

/// Where our vector table lives. Must match ORIGIN(FLASH) in memory.x. Setting
/// VTOR explicitly means a warm reset from any prior state still lands here.
const APP_BASE: u32 = 0x0800_0000;

/// Running on the internal 16 MHz oscillator - no PLL, no external crystal.
const SYSCLK_HZ: u32 = 16_000_000;

const HEADLINE_FONT: MonoFont = FONT_9X15_BOLD;
const BODY_FONT: MonoFont = FONT_6X10;
const HEADLINE_GAP: i32 = 5;
const LINE_PITCH: i32 = 12;

const PANEL_W: i32 = 128;

/// This panel is a two-tone module: 16 rows of yellow and 48 of blue, with a
/// hard colour break where they meet. After Rotate180 the yellow band is the
/// bottom 16 rows in our coordinates, so artwork is centred within the blue
/// region only - otherwise a line comes out half yellow and half blue.
const BLUE_ROWS: i32 = 48;

/// Ferris gets drawn at 2x. A 1bpp bitmap needs its rows padded to whole bytes;
/// 48 * 2 = 96 pixels is exactly 12 bytes, so there is no padding to worry about.
const SPRITE_SCALE: usize = 2;
const SPRITE_W: usize = FERRIS_W * SPRITE_SCALE;
const SPRITE_H: usize = FERRIS_H * SPRITE_SCALE;
const SPRITE_STRIDE: usize = SPRITE_W / 8;
const SPRITE_LEN: usize = SPRITE_STRIDE * SPRITE_H;

#[entry]
fn main() -> ! {
    // Must happen before anything can fault or take an interrupt.
    unsafe {
        (*cortex_m::peripheral::SCB::PTR).vtor.write(APP_BASE);
    }

    let dp = pac::Peripherals::take().unwrap();
    let clocks = dp.RCC.constrain().cfgr.freeze();

    // The panel's RES# line is wired to PA8. Out of a cold reset PA8 is a
    // floating input, which leaves the SSD1306 held in reset - it powers up but
    // never ACKs its I2C address. MXChip's bootloader used to release this for
    // us; booting directly, it is on us. Pulse it low, then leave it high.
    let gpioa = dp.GPIOA.split();
    let mut oled_reset = gpioa.pa8.into_push_pull_output();
    oled_reset.set_low();
    sleep_ms(10);
    oled_reset.set_high();
    sleep_ms(100);

    let gpiob = dp.GPIOB.split();
    let scl = gpiob.pb8.into_alternate_open_drain();
    let sda = gpiob.pb9.into_alternate_open_drain();
    let i2c = dp.I2C1.i2c((scl, sda), 400.kHz(), &clocks);

    // The panel is mounted with segment-remap + inverted COM scan, so it reads
    // the right way up only when the driver rotates by 180 degrees.
    let mut display = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate180,
    )
    .into_buffered_graphics_mode();

    // Retry rather than panic: a gift that freezes on a cold-start race would
    // be a sad gift. If the panel is slow to come out of reset, just try again.
    while display.init().is_err() {
        sleep_ms(200);
    }

    let mut sprite = [0u8; SPRITE_LEN];
    expand_ferris(&mut sprite);
    let ferris = ImageRaw::<BinaryColor>::new(&sprite, SPRITE_W as u32);
    let ferris_at = Point::new(
        (PANEL_W - SPRITE_W as i32) / 2,
        (BLUE_ROWS - SPRITE_H as i32) / 2,
    );

    let headline_style = MonoTextStyle::new(&HEADLINE_FONT, BinaryColor::On);
    let body_style = MonoTextStyle::new(&BODY_FONT, BinaryColor::On);
    let centered = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .baseline(Baseline::Top)
        .build();
    let border = PrimitiveStyle::with_stroke(BinaryColor::On, 1);

    loop {
        for card in CARDS {
            display.clear(BinaryColor::Off).unwrap();

            match card {
                // No frame on this one - Ferris runs to both edges already.
                Card::Ferris => {
                    Image::new(&ferris, ferris_at).draw(&mut display).unwrap();
                }
                Card::Text(headline, lines) => {
                    // Frame the blue region only. Stretching it over all 64 rows
                    // would light pixels under the yellow stripe, which is the
                    // only reason any yellow ever showed up on a text card.
                    Rectangle::new(Point::zero(), Size::new(PANEL_W as u32, BLUE_ROWS as u32))
                        .into_styled(border)
                        .draw(&mut display)
                        .unwrap();

                    let mut y = top_of_block(headline, lines);

                    if !headline.is_empty() {
                        Text::with_text_style(
                            headline,
                            Point::new(PANEL_W / 2, y),
                            headline_style,
                            centered,
                        )
                        .draw(&mut display)
                        .unwrap();
                        y += HEADLINE_FONT.character_size.height as i32 + HEADLINE_GAP;
                    }

                    for line in *lines {
                        Text::with_text_style(
                            line,
                            Point::new(PANEL_W / 2, y),
                            body_style,
                            centered,
                        )
                        .draw(&mut display)
                        .unwrap();
                        y += LINE_PITCH;
                    }
                }
            }

            // A dropped frame is not worth halting the whole card for.
            let _ = display.flush();
            sleep_ms(CARD_DWELL_MS);
        }
    }
}

/// Blow the ASCII art up to 2x into a 1bpp, MSB-first bitmap. Every index is
/// bounded by the constants above, so a mis-sized row clips instead of panicking.
fn expand_ferris(buf: &mut [u8; SPRITE_LEN]) {
    for (sy, row) in FERRIS.iter().enumerate() {
        if sy >= FERRIS_H {
            break;
        }
        for (sx, ch) in row.bytes().enumerate() {
            if sx >= FERRIS_W {
                break;
            }
            if ch != b'#' {
                continue;
            }
            for dy in 0..SPRITE_SCALE {
                for dx in 0..SPRITE_SCALE {
                    let x = sx * SPRITE_SCALE + dx;
                    let y = sy * SPRITE_SCALE + dy;
                    buf[y * SPRITE_STRIDE + x / 8] |= 0x80 >> (x % 8);
                }
            }
        }
    }
}

/// Vertically centre the card's block of text within the blue part of the panel.
fn top_of_block(headline: &str, lines: &[&str]) -> i32 {
    let mut height =
        lines.len() as i32 * LINE_PITCH - (LINE_PITCH - BODY_FONT.character_size.height as i32);
    if !headline.is_empty() {
        height += HEADLINE_FONT.character_size.height as i32 + HEADLINE_GAP;
    }
    let top = (BLUE_ROWS - height) / 2;
    if top < 3 {
        3
    } else {
        top
    }
}

/// Busy-wait. Accurate enough for a display loop and depends on no HAL timer API.
fn sleep_ms(ms: u32) {
    cortex_m::asm::delay(SYSCLK_HZ / 1_000 * ms);
}
