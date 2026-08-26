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
    // iso_8859_1 rather than ascii: the ascii set has no glyph for the diaeresis
    // in "Joël" and would draw a placeholder box instead.
    mono_font::{
        iso_8859_1::{FONT_6X10, FONT_9X15_BOLD},
        MonoFont, MonoTextStyle,
    },
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle, Triangle},
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
    /// Big lines (9x15 bold) followed by body lines (6x10). Either list may be
    /// empty. Width budget: 14 characters big, 21 characters body.
    Text(&'static [&'static str], &'static [&'static str]),
    /// A hazard triangle over body lines. No font here has a warning glyph, so
    /// the sign is drawn. Two body lines is the most that fits under it.
    Warning(&'static [&'static str]),
    /// Ferris, the Rust mascot, drawn from the ASCII art further down.
    Ferris,
}

static CARDS: &[Card] = &[
    Card::Warning(&["Emotional farewell", "embedded style ahead"]),
    Card::Text(&["Joël", "THANK YOU"], &[]),
    Card::Text(&[], &["for everything", "you built with us", "at Q*Bird"]),
    Card::Text(&["3.5 YEARS"], &["of working together!"]),
    Card::Text(&["2 RUST TALKS"], &["given by you:", "12/12/24 & 27/08/26"]),
    Card::Text(&["GOOD LUCK"], &["on your next", "adventure"]),
    Card::Text(&["Ahmed"], &["& the Q*Bird team"]),
    Card::Text(&[], &["(this card runs on", "128x64 px of Rust)"]),
    Card::Ferris,
];

/// How long each card stays on screen.
const CARD_DWELL_MS: u32 = 2_500;

/// One colour for the RGB LED per card. Each card switch flares the LED up to
/// full in its colour and then settles to a low glow, so the change is
/// announced rather than just happening. Indexed modulo, so a short palette
/// simply repeats and you can never index past the end.
static PALETTE: &[(u8, u8, u8)] = &[
    (255, 130, 0),   // warning        - amber
    (255, 215, 160), // Joël/THANK YOU - warm white
    (0, 190, 255),   // at Q*Bird      - cyan
    (0, 255, 90),    // 3.5 YEARS      - green
    (255, 70, 0),    // 2 RUST TALKS   - rust orange
    (60, 120, 255),  // GOOD LUCK      - blue
    (225, 0, 255),   // Ahmed          - magenta
    (255, 255, 255), // 128x64 px      - white
    (255, 95, 25),   // Ferris         - crab orange
];

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

const HEADLINE_H: i32 = HEADLINE_FONT.character_size.height as i32;
const BODY_H: i32 = BODY_FONT.character_size.height as i32;

/// Baseline-to-baseline spacing within each run of lines, and the larger gap
/// where a run of big lines meets a run of body lines.
const HEADLINE_PITCH: i32 = HEADLINE_H + 2;
const LINE_PITCH: i32 = BODY_H + 2;
const HEADLINE_GAP: i32 = 5;

/// The hazard triangle on the warning card, sized so the sign plus two body
/// lines still clears the yellow seam.
const WARN_H: i32 = 16;
const WARN_HALF_W: i32 = 11;

const PANEL_W: i32 = 128;

/// This panel is a two-tone module: 16 rows of yellow and 48 of blue, with a
/// hard colour break where they meet. After Rotate180 the yellow band is the
/// bottom 16 rows in our coordinates, so artwork is centred within the blue
/// region only - otherwise a line comes out half yellow and half blue.
const BLUE_ROWS: i32 = 48;

/// The yellow stripe is the one part of the panel that isn't blue, so it gets
/// the progress bar: it fills across the whole deck and resets when the loop
/// restarts, which reads as both "how far through" and "how long until the next
/// card". Redrawing only these rows keeps each frame cheap - ssd1306 tracks the
/// dirty region, so a step flushes two pages rather than the whole framebuffer.
const PROGRESS_TOP: i32 = 52;
const PROGRESS_H: u32 = 8;
const PROGRESS_INSET: i32 = 4;

/// Animation steps per card. 40 across 2.5 s is a visible crawl rather than a jump.
const PROGRESS_STEPS: u32 = 40;

/// The RGB LED flare: rise to full over RISE steps, ease back to IDLE over FALL,
/// then hold. In steps, against PROGRESS_STEPS above.
const LED_RISE: u32 = 5;
const LED_FALL: u32 = 14;
const LED_PEAK: u32 = 255;
const LED_IDLE: u32 = 30;

/// 1 kHz is far above anything the eye catches, and with a 16 MHz timer clock it
/// leaves ~16000 steps of duty resolution - plenty for a smooth fade.
const LED_PWM_HZ: u32 = 1_000;

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

    // Costs nothing at runtime, but makes COLOPHON a genuine reference so the
    // linker keeps the note in the flashed image. See the bottom of this file.
    core::hint::black_box(COLOPHON);

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
    let gpioc = dp.GPIOC.split();

    let scl = gpiob.pb8.into_alternate_open_drain();
    let sda = gpiob.pb9.into_alternate_open_drain();
    let i2c = dp.I2C1.i2c((scl, sda), 400.kHz(), &clocks);

    // RGB LED. Red and blue hang off TIM3, green off TIM2 - that split is the
    // board's wiring, not a choice. PB3/PB4 come out of reset as JTAG pins
    // (JTDO/NJTRST); reconfiguring them is harmless because we debug over SWD.
    //
    // Hardware PWM rather than bit-banging: the timers keep driving the LED
    // while the CPU is blocked pushing pixels over I2C, which software PWM
    // would visibly stutter through.
    let (_tim3, (t3c1, t3c2, ..)) = dp.TIM3.pwm_hz(LED_PWM_HZ.Hz(), &clocks);
    let (_tim2, (_, t2c2, ..)) = dp.TIM2.pwm_hz(LED_PWM_HZ.Hz(), &clocks);
    // PB4/PB3 arrive as Alternate<0>, i.e. still in JTAG mode, and the HAL will
    // only take a pin that isn't already alternate - so claim them as plain
    // outputs first. PC7 needs no such thing.
    let mut led_r = t3c1.with(gpiob.pb4.into_push_pull_output());
    let mut led_b = t3c2.with(gpioc.pc7);
    let mut led_g = t2c2.with(gpiob.pb3.into_push_pull_output());
    led_r.enable();
    led_g.enable();
    led_b.enable();
    let led_max = (led_r.get_max_duty(), led_g.get_max_duty(), led_b.get_max_duty());

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

    let total_steps = CARDS.len() as u32 * PROGRESS_STEPS;

    loop {
        for (index, card) in CARDS.iter().enumerate() {
            display.clear(BinaryColor::Off).unwrap();

            match card {
                // No frame on this one - Ferris runs to both edges already.
                Card::Ferris => {
                    Image::new(&ferris, ferris_at).draw(&mut display).unwrap();
                }
                Card::Warning(lines) => {
                    Rectangle::new(Point::zero(), Size::new(PANEL_W as u32, BLUE_ROWS as u32))
                        .into_styled(border)
                        .draw(&mut display)
                        .unwrap();

                    let text_h = (lines.len() as i32 - 1) * LINE_PITCH + BODY_H;
                    let mut y = (BLUE_ROWS - (WARN_H + HEADLINE_GAP + text_h)) / 2;
                    if y < 3 {
                        y = 3;
                    }

                    draw_warning_sign(&mut display, y);
                    y += WARN_H + HEADLINE_GAP;

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
                Card::Text(big, lines) => {
                    // Frame the blue region only. Stretching it over all 64 rows
                    // would light pixels under the yellow stripe, which is the
                    // only reason any yellow ever showed up on a text card.
                    Rectangle::new(Point::zero(), Size::new(PANEL_W as u32, BLUE_ROWS as u32))
                        .into_styled(border)
                        .draw(&mut display)
                        .unwrap();

                    let mut y = top_of_block(big, lines);

                    for line in *big {
                        Text::with_text_style(
                            line,
                            Point::new(PANEL_W / 2, y),
                            headline_style,
                            centered,
                        )
                        .draw(&mut display)
                        .unwrap();
                        y += HEADLINE_PITCH;
                    }

                    // The loop above left y a full pitch past the last big line;
                    // back up to its bottom edge and open the wider gap instead.
                    if !big.is_empty() {
                        y += HEADLINE_H + HEADLINE_GAP - HEADLINE_PITCH;
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

            // Hold the card, creeping the progress bar along as we wait. Only
            // the yellow rows change per step, so these flushes are small; the
            // first one carries the whole card because clear() dirtied it all.
            let colour = PALETTE[index % PALETTE.len()];

            for step in 0..PROGRESS_STEPS {
                let level = led_level(step);
                led_r.set_duty(scale_duty(led_max.0, colour.0, level));
                led_g.set_duty(scale_duty(led_max.1, colour.1, level));
                led_b.set_duty(scale_duty(led_max.2, colour.2, level));

                draw_progress(
                    &mut display,
                    index as u32 * PROGRESS_STEPS + step + 1,
                    total_steps,
                );
                // A dropped frame is not worth halting the whole card for.
                let _ = display.flush();
                sleep_ms(CARD_DWELL_MS / PROGRESS_STEPS);
            }
        }
    }
}

/// Brightness envelope for one card, in 0..=LED_PEAK: flare to full as the card
/// appears, ease down, then hold a low glow until the next one.
fn led_level(step: u32) -> u32 {
    if step < LED_RISE {
        LED_PEAK * (step + 1) / LED_RISE
    } else if step < LED_RISE + LED_FALL {
        LED_PEAK - (LED_PEAK - LED_IDLE) * (step - LED_RISE) / LED_FALL
    } else {
        LED_IDLE
    }
}

/// Duty for one LED channel, squared so the fade looks linear to the eye rather
/// than rushing the top end. Every intermediate stays well inside u32.
fn scale_duty(max: u16, channel: u8, level: u32) -> u16 {
    let gamma = level * level / LED_PEAK;
    ((max as u32 * channel as u32 / 255) * gamma / LED_PEAK) as u16
}

/// A hazard triangle with an exclamation mark, drawn rather than typed - the
/// bar and dot are sized to sit inside the triangle where it is still wide
/// enough to hold them.
fn draw_warning_sign<D>(target: &mut D, top: i32)
where
    D: DrawTarget<Color = BinaryColor>,
{
    let cx = PANEL_W / 2;
    let bottom = top + WARN_H - 1;

    let _ = Triangle::new(
        Point::new(cx, top),
        Point::new(cx - WARN_HALF_W, bottom),
        Point::new(cx + WARN_HALF_W, bottom),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(target);

    let _ = Rectangle::new(Point::new(cx - 1, top + 5), Size::new(2, 5))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(target);
    let _ = Rectangle::new(Point::new(cx - 1, top + 12), Size::new(2, 2))
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(target);
}

/// Draw the deck-progress bar into the yellow stripe. Wipes the band first so
/// successive steps don't smear, and touches nothing above BLUE_ROWS.
fn draw_progress<D>(target: &mut D, done: u32, total: u32)
where
    D: DrawTarget<Color = BinaryColor>,
{
    let track_w = (PANEL_W - 2 * PROGRESS_INSET) as u32;

    let _ = Rectangle::new(
        Point::new(0, BLUE_ROWS),
        Size::new(PANEL_W as u32, (64 - BLUE_ROWS) as u32),
    )
    .into_styled(PrimitiveStyle::with_fill(BinaryColor::Off))
    .draw(target);

    let _ = Rectangle::new(
        Point::new(PROGRESS_INSET, PROGRESS_TOP),
        Size::new(track_w, PROGRESS_H),
    )
    .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1))
    .draw(target);

    let filled = (track_w - 2) * done / total;
    if filled > 0 {
        let _ = Rectangle::new(
            Point::new(PROGRESS_INSET + 1, PROGRESS_TOP + 1),
            Size::new(filled, PROGRESS_H - 2),
        )
        .into_styled(PrimitiveStyle::with_fill(BinaryColor::On))
        .draw(target);
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
fn top_of_block(big: &[&str], lines: &[&str]) -> i32 {
    let mut height = 0;
    if !big.is_empty() {
        height += (big.len() as i32 - 1) * HEADLINE_PITCH + HEADLINE_H;
    }
    if !lines.is_empty() {
        if height > 0 {
            height += HEADLINE_GAP;
        }
        height += (lines.len() as i32 - 1) * LINE_PITCH + BODY_H;
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

// ---------------------------------------------------------------------------

/// Never drawn on the panel. It is in the flashed image though, so:
///
///     strings firmware.bin | grep -A20 'started at'
///
/// Keeping it there takes more than `#[used]`: that stops LLVM dropping the
/// static, but the linker still runs --gc-sections and collects it anyway. The
/// black_box() in main() is what actually holds it in - it makes the string
/// genuinely referenced by code, so the section survives.
#[used]
static COLOPHON: &str = "
This started at 11:12pm on 26 August 2026, as 'I have an hour, and if it
doesn't work in an hour I'm not doing it'. It did not work in an hour. It
went past midnight, mostly because the bootloader fought back twice.

Round one: flashing the ELF instead of the .bin wrote a stray segment to
0x08000000, erased flash sector 0 and took MXChip's bootloader with it.
Recovered from Microsoft's own release image. Round two: the restored
bootloader then flatly refused to chain-load a non-MXChip app, so this
firmware gave up on it and took over flash from the base address instead.

The one that actually cost the evening: this panel's OLED will not answer
on I2C at all until you release its reset line on PA8. Out of a cold boot
that pin floats, the display is held in reset, and you get a perfectly
configured I2C bus talking to nobody. It isn't in any pinout diagram - it
is one line in Zephyr's devicetree.

Ahmed wrote it. Claude (Opus 5) did the driving on the STM32 side: found
the pinout, bricked the bootloader, put it back, and worked out the PA8
thing by dumping registers over SWD until the silence made sense.

The crab is hand-drawn ASCII, 64x22, scaled 2x at boot. The progress bar
lives in the bottom 16 rows because that strip of the glass is physically
yellow instead of blue, and it seemed a waste not to use it.

Thanks for everything, Joël.
";
