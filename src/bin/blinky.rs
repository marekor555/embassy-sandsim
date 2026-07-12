#![no_std]
#![no_main]
extern crate alloc;
use embedded_alloc::LlffHeap as Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();

use embassy_executor::Spawner;
use embassy_nrf::{gpio, twim, bind_interrupts, peripherals};
use embassy_nrf::saadc;
use embassy_time::Timer; // delays etc.

use {defmt_rtt as _, panic_probe as _}; // for info! macro
use defmt::{info};
use embassy_nrf::saadc::ChannelConfig;
use embassy_nrf::twim::Twim;
use embedded_graphics::{
    mono_font::{ascii::FONT_6X13, MonoTextStyle},
    pixelcolor::BinaryColor,
    primitives::{Circle, Primitive, PrimitiveStyle},
    prelude::*,
};
use embedded_graphics::text::Text;
use ssd1306::{
    I2CDisplayInterface,
    Ssd1306,
    prelude::DisplayRotation,
    prelude::*,
    size::DisplaySize128x32,
};
use static_cell::StaticCell;

use lsm6ds3tr;
use lsm6ds3tr::interface::I2cInterface;
use lsm6ds3tr::LsmSettings;

use rand::{SeedableRng, Rng};

bind_interrupts!(struct IrqsDisplay {
    SERIAL22 => twim::InterruptHandler<peripherals::SERIAL22>;
});

bind_interrupts!(struct IrqsImu {
    SERIAL30 => twim::InterruptHandler<peripherals::SERIAL30>;
});

bind_interrupts!(struct IrqsSaadc {
    SAADC => saadc::InterruptHandler;
});


fn clamp(x: i16, min: i16, max: i16) -> i16 {
    if x < min {
        return min;
    } else if x > max {
        return max;
    }

    x
}

fn invalid_position(x: i16, y: i16, pixels: &[[bool; 32]; 128], new_pixels: &[[bool; 32]; 128]) -> bool {
    pixels[x as usize][y as usize] || new_pixels[x as usize][y as usize]
}

fn conway(pixels: &mut[[bool; 32]; 128]){
    let mut new_pixels = [[false; 32]; 128];

    for x in 1..127 {
        for y in 1..31 {
            let mut count = 0;
            for dx in -1..2 {
                for dy in -1..2 {
                    if dx == 0 && dy == 0 {continue}
                    count += if pixels[(x+dx) as usize][(y+dy) as usize] {1} else {0};
                }
            }
            if count == 3 || (count == 2 && pixels[x as usize][y as usize]) {
                new_pixels[x as usize][y as usize] = true;
            }
        }
    }
    *pixels = new_pixels;
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    { // init heap
        use core::mem::MaybeUninit;
        static mut HEAP_MEM: [MaybeUninit<u8>; 8192] = [MaybeUninit::uninit(); 8192];
        unsafe {
            HEAP.init(&raw mut HEAP_MEM as usize, 8192);
        }
    }
    let p = embassy_nrf::init(Default::default()); // init pins


    // display init
    let mut config_display = twim::Config::default();
    config_display.frequency = twim::Frequency::K1000;

    static RAM_BUF_DISP: StaticCell<[u8; 128]> = StaticCell::new();
    let i2c_display = Twim::new(p.SERIAL22, IrqsDisplay, p.P1_10, p.P1_11, config_display, RAM_BUF_DISP.init([0; 128]));
    let display_interface = I2CDisplayInterface::new(i2c_display);

    let mut display: Ssd1306<_, DisplaySize128x32, _>
        = Ssd1306::new(display_interface, DisplaySize128x32, DisplayRotation::Rotate0).into_buffered_graphics_mode();
    info!("display init");
    display.init().expect("display.init failed");
    info!("display init done");

    let style = MonoTextStyle::new(&FONT_6X13, BinaryColor::On);
    let style2 = PrimitiveStyle::with_stroke(BinaryColor::On, 1);
    let style3 = PrimitiveStyle::with_stroke(BinaryColor::Off, 1);

    // imu init

    // power-cycle the imu
    let _imu_pwr = gpio::Output::new(p.P0_01, gpio::Level::High, gpio::OutputDrive::Standard);
    Timer::after_millis(50).await;

    let mut config_imu = twim::Config::default();
    config_imu.frequency = twim::Frequency::K400;

    static RAM_BUF_IMU: StaticCell<[u8; 128]> = StaticCell::new();
    let i2c_imu = Twim::new(
        p.SERIAL30,
        IrqsImu,
        p.P0_04,
        p.P0_03,
        config_imu,
        RAM_BUF_IMU.init([0; 128]),
    );

    let imu_interface = I2cInterface::new(i2c_imu);
    let settings = LsmSettings::basic();
    let mut imu = lsm6ds3tr::LSM6DS3TR::new(imu_interface).with_settings(settings);

    info!("imu init");
    imu.init().expect("imu.init failed");
    info!("imu init done");

    let mut rng = rand::rngs::SmallRng::seed_from_u64(0);

    info!("filling display");
    display.fill_solid(&display.bounding_box(), BinaryColor::Off).unwrap();
    Text::new("HELLO WORLD!", Point::new(0, 13), style).draw(&mut display).unwrap();
    display.flush().unwrap();
    info!("All done");



    let btn = gpio::Input::new(p.P2_07, gpio::Pull::Up);
    let btn2 = gpio::Input::new(p.P2_01, gpio::Pull::Up);
    let mut btn3 = gpio::Input::new(p.P2_04, gpio::Pull::Up);
    let btn4 = gpio::Input::new(p.P0_00, gpio::Pull::Up);

    info!("saadc");
    let mut saadc = saadc::Saadc::new(
        p.SAADC,
        IrqsSaadc,
        saadc::Config::default(),
        [
            ChannelConfig::single_ended(p.P1_04),
            ChannelConfig::single_ended(p.P1_05),
        ],
    );

    // pixels in the simulation
    let mut pixels : [[bool; 32]; 128] = [[false; 32]; 128];

    for x in 0..128 {
        for y in 0..32 {
            if rng.random_bool(0.25) {
                pixels[x][y] = true;
            }
        }
    }

    let mut buf = [0i16; 2];

    let mut pos_x = 64;
    let mut pos_y = 16;

    let mut conway_run = false;

    info!("start");
    loop {
        if (btn4.is_low()) {
            for x in 0..128 {
                for y in 0..32 {
                    if rng.random_bool(0.25) {
                        pixels[x][y] = true;
                    } else {
                        pixels[x][y] = false;
                    }
                }
            }
            Timer::after_millis(1000).await;
        }

        if (btn3.is_low()) {
            conway_run = !conway_run;
            info!("switch");
            Timer::after_millis(500).await;
        }

        if (conway_run) {
            conway(&mut pixels);
        } else {
            let accel = imu.read_accel().unwrap();

            info!("\nx: {}\ny: {}\nz: {}\n", accel.x, accel.y, accel.z);

            let mut new_pixels: [[bool; 32]; 128] = [[false; 32]; 128];
            for xr in 0..128 {
                let x = if xr % 2 == 0 { xr / 2 } else { 127 - xr / 2 };
                for yr in 0..32 {
                    let y = if yr % 2 == 0 { yr / 2 } else { 31 - yr / 2 };
                    if pixels[x as usize][y as usize] {
                        let accel_x = -(accel.x * 5.0) as i16;
                        let accel_y = (accel.y * 5.0) as i16;
                        let new_x = clamp(x + accel_x, 0, 127);
                        let new_y = clamp(y + accel_y, 0, 31);

                        if invalid_position(new_x, new_y, &pixels, &new_pixels) {
                            let new_x = if accel_x != 0 { clamp(x + if accel_x > 0 { 1 } else { -1 }, 0, 127) } else { x };
                            let new_y = if accel_y != 0 { clamp(y + if accel_y > 0 { 1 } else { -1 }, 0, 31) } else { y };

                            let randdir = rng.random_range(-1..=1);
                            if invalid_position(x, new_y, &pixels, &new_pixels) && new_y != y && !invalid_position(clamp(x - randdir, 0, 127), y, &pixels, &new_pixels) {
                                new_pixels[clamp(x - randdir, 0, 127) as usize][y as usize] = true;
                                continue;
                            } else if invalid_position(new_x, y, &pixels, &new_pixels) && new_x != x && !invalid_position(x, clamp(y - randdir, 0, 31), &pixels, &new_pixels) {
                                new_pixels[x as usize][clamp(y - randdir, 0, 31) as usize] = true;
                                continue;
                            }

                            if invalid_position(new_x, new_y, &pixels, &new_pixels) {
                                new_pixels[x as usize][y as usize] = true;
                                continue;
                            }
                            new_pixels[new_x as usize][new_y as usize] = true;
                            continue;
                        }
                        new_pixels[new_x as usize][new_y as usize] = true;
                    }
                }
            }
            pixels = new_pixels;
        }


        saadc.sample(&mut buf).await;
        info!("\nx: {}\ny: {}\n", buf[0]/700 - 2, buf[1]/700 - 2);

        let dx = clamp(buf[0] / 700 - 2, -2, 2);
        let dy = -clamp(buf[1] / 700 - 2, -2, 2);

        pos_x = clamp(pos_x + dx, 0 + 5, 127 - 4);
        pos_y = clamp(pos_y + dy, 0 + 5, 31 - 4);

        for x in -5..5 {
            for y in -5..5 {
                if btn.is_low() {
                    pixels[(pos_x + x) as usize][(pos_y + y) as usize] = true;
                }
                if btn2.is_low() {
                    pixels[(pos_x + x) as usize][(pos_y + y) as usize] = false;
                }
            }
        }

        display.fill_solid(&display.bounding_box(), BinaryColor::Off).unwrap();

        for x in 0..128 {
            for y in 0..32 {
                display.set_pixel(x, y, pixels[x as usize][y as usize]);
            }
        }

        for r in 1..4 {
            Circle::new(Point::new((pos_x-r) as i32, (pos_y-r) as i32), (r*2) as u32).into_styled(if r % 2 == 0 {style2} else {style3}).draw(&mut display).unwrap();
        }

        display.flush().unwrap();

        Timer::after_millis(1).await;
    }
}