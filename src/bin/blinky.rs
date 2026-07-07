#![no_std]
#![no_main]
extern crate alloc;
use embedded_alloc::LlffHeap as Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();

use embassy_executor::Spawner;
use embassy_nrf::{gpio, twim, bind_interrupts, peripherals};
use embassy_time::Timer; // delays etc.

use {defmt_rtt as _, panic_probe as _}; // for info! macro
use defmt::{info};
use embassy_nrf::twim::Twim;
use embedded_graphics::{
    mono_font::{ascii::FONT_6X13, MonoTextStyle},
    pixelcolor::BinaryColor,
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


fn clamp(x: i16, min: i16, max: i16) -> i16 {
    if x < min {
        return min;
    } else if x > max {
        return max;
    }

    x
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
    display.init().expect("display.init failed");
    let style = MonoTextStyle::new(&FONT_6X13, BinaryColor::On);

    // imu init
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
    imu.init().expect("imu.init failed");

    let mut rng = rand::rngs::SmallRng::seed_from_u64(0);

    display.fill_solid(&display.bounding_box(), BinaryColor::Off).unwrap();
    Text::new("HELLO WORLD!", Point::new(0, 13), style).draw(&mut display).unwrap();
    display.flush().unwrap();
    Timer::after_millis(500).await;


    let btn = gpio::Input::new(p.P2_07, gpio::Pull::Up);

    // pixels in the simulation
    let mut pixels : [[bool; 32]; 128] = [[false; 32]; 128];

    for x in 0..128 {
        for y in 0..32 {
            if rng.random_bool(0.25) {
                pixels[x][y] = true;
            }
        }
    }

    loop {
        let accel = imu.read_accel().unwrap();

        info!("x: {}, y: {}, z: {}", (accel.x*5.0) as i8, (accel.y*5.0) as i8, (accel.z*5.0) as i8);

        let mut new_pixels : [[bool; 32]; 128] = [[false; 32]; 128];
        for x in 0..128 {
            for y in 0..32 {
                if (pixels[x as usize][y as usize]) {
                    let accel_x = (accel.x*5.0) as i16;
                    let accel_y = (accel.y*5.0) as i16;
                    let new_x = clamp(x as i16 - accel_x, 0, 127);
                    let new_y = clamp(y as i16 + accel_y as i16, 0, 31);

                    if (pixels[new_x as usize][new_y as usize] || new_pixels[new_x as usize][new_y as usize]) {
                        let new_x = if accel_x != 0 {clamp(x - if accel_x > 0 { 1 } else { -1 }, 0, 127)} else {x};
                        let new_y = if accel_y != 0 {clamp(y + if accel_y > 0 { 1 } else { -1 }, 0, 31)} else {y};
                        if (pixels[new_x as usize][new_y as usize] || new_pixels[new_x as usize][new_y as usize]) {
                            new_pixels[x as usize][y as usize] = true;
                        } else {
                            new_pixels[new_x as usize][new_y as usize] = true;
                        }
                    } else {
                        new_pixels[new_x as usize][new_y as usize] = true;
                    }
                }
            }
        }

        if btn.is_low(){
            for x in -5..5 {
                for y in -5..5 {
                    new_pixels[(64 + x) as usize][(16 + y) as usize] = true;
                }
            }
        }

        pixels = new_pixels;

        display.fill_solid(&display.bounding_box(), BinaryColor::Off).unwrap();

        for x in 0..128 {
            for y in 0..32 {
                display.set_pixel(x, y, pixels[x as usize][y as usize]);
            }
        }

        display.flush().unwrap();

        Timer::after_millis(1).await;
    }
}