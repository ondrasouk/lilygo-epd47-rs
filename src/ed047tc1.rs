use core::ptr::addr_of_mut;

use esp_hal::clock::Clocks;
use esp_hal::gpio::{GpioPin, Io, Level, Output, OutputPin};
use esp_hal::lcd_cam::lcd::i8080;
use esp_hal::lcd_cam::LcdCam;
use esp_hal::peripheral::Peripheral;
use esp_hal::prelude::_fugit_RateExtU32;
use esp_hal::{dma, peripherals};

use crate::{rmt, BYTES_PER_LINE};

/*
/* Config Reggister Control */
#define CFG_DATA GPIO_NUM_13
#define CFG_CLK GPIO_NUM_12
#define CFG_STR GPIO_NUM_0

/* Control Lines */
#define CKV GPIO_NUM_38
#define STH GPIO_NUM_40

/* Edges */
#define CKH GPIO_NUM_41

/* Data Lines */
#define D7 GPIO_NUM_7
#define D6 GPIO_NUM_6
#define D5 GPIO_NUM_5
#define D4 GPIO_NUM_4
#define D3 GPIO_NUM_3
#define D2 GPIO_NUM_2
#define D1 GPIO_NUM_1
#define D0 GPIO_NUM_8
 */

static mut TX_DESCRIPTORS: [dma::DmaDescriptor; 1] = [dma::DmaDescriptor::EMPTY; 1];
static mut RX_DESCRIPTORS: [dma::DmaDescriptor; 0] = [dma::DmaDescriptor::EMPTY; 0];

const DMA_BUFFER_SIZE: usize = 248;

fn dma_buffer() -> &'static mut [u8; DMA_BUFFER_SIZE] {
    static mut BUFFER: [u8; DMA_BUFFER_SIZE] = [0u8; DMA_BUFFER_SIZE];
    unsafe { &mut *addr_of_mut!(BUFFER) }
}

struct ConfigRegister {
    latch_enable: bool,
    power_disable: bool,
    pos_power_enable: bool,
    neg_power_enable: bool,
    stv: bool,
    scan_direction: bool,
    mode: bool,
    output_enable: bool,
}

impl Default for ConfigRegister {
    fn default() -> Self {
        ConfigRegister {
            latch_enable: false,
            power_disable: true,
            pos_power_enable: false,
            neg_power_enable: false,
            stv: true,
            scan_direction: true,
            mode: false,
            output_enable: false,
        }
    }
}

struct ConfigWriter<'a, DATA, CLK, STR>
where
    DATA: OutputPin,
    CLK: OutputPin,
    STR: OutputPin,
{
    pin_data: Output<'a, DATA>,
    pin_clk: Output<'a, CLK>,
    pin_str: Output<'a, STR>,
    config: ConfigRegister,
}

impl<'a, DATA, CLK, STR> ConfigWriter<'a, DATA, CLK, STR>
where
    DATA: OutputPin,
    CLK: OutputPin,
    STR: OutputPin,
{
    fn new(
        data: impl Peripheral<P = DATA> + 'a,
        clk: impl Peripheral<P = CLK> + 'a,
        str: impl Peripheral<P = STR> + 'a,
    ) -> Self {
        ConfigWriter {
            pin_data: Output::new(data, Level::High),
            pin_clk: Output::new(clk, Level::High),
            pin_str: Output::new(str, Level::Low),
            config: ConfigRegister::default(),
        }
    }

    fn write(&mut self) {
        self.pin_str.set_low();
        self.write_bool(self.config.output_enable);
        self.write_bool(self.config.mode);
        self.write_bool(self.config.scan_direction);
        self.write_bool(self.config.stv);
        self.write_bool(self.config.neg_power_enable);
        self.write_bool(self.config.pos_power_enable);
        self.write_bool(self.config.power_disable);
        self.write_bool(self.config.latch_enable);
        self.pin_str.set_high();
    }

    #[inline(always)]
    fn write_bool(&mut self, v: bool) {
        self.pin_clk.set_low();
        self.pin_data.set_level(match v {
            true => Level::High,
            false => Level::Low,
        });
        self.pin_clk.set_high();
    }
}

pub(crate) struct ED047TC1<'a> {
    i8080: i8080::I8080<
        'a,
        dma::ChannelTx<'a, dma::ChannelTxImpl<0>, dma::Channel0>,
        i8080::TxEightBits<
            'a,
            GpioPin<6>,
            GpioPin<7>,
            GpioPin<4>,
            GpioPin<5>,
            GpioPin<2>,
            GpioPin<3>,
            GpioPin<8>,
            GpioPin<1>,
        >,
    >,
    cfg_writer: ConfigWriter<'a, GpioPin<13>, GpioPin<12>, GpioPin<0>>,
    rmt: rmt::Rmt<'a>,
}

impl<'a> ED047TC1<'a> {
    pub(crate) fn new(
        io: Io,
        dma: impl Peripheral<P = peripherals::DMA> + 'a,
        lcd_cam: impl Peripheral<P = peripherals::LCD_CAM> + 'a,
        rmt: impl Peripheral<P = peripherals::RMT> + 'a,
        clocks: &'a Clocks,
    ) -> Self {
        // configure data pins
        let tx_pins = i8080::TxEightBits::new(
            io.pins.gpio6,
            io.pins.gpio7,
            io.pins.gpio4,
            io.pins.gpio5,
            io.pins.gpio2,
            io.pins.gpio3,
            io.pins.gpio8,
            io.pins.gpio1,
        );

        // configure dma
        let dma = dma::Dma::new(dma);
        let channel = unsafe {
            dma.channel0.configure(
                false,
                &mut *addr_of_mut!(TX_DESCRIPTORS),
                &mut *addr_of_mut!(RX_DESCRIPTORS),
                dma::DmaPriority::Priority0,
            )
        };

        // init lcd
        let lcd_cam = LcdCam::new(lcd_cam);

        // init panel config writer (?)
        let mut cfg_writer = ConfigWriter::new(io.pins.gpio13, io.pins.gpio12, io.pins.gpio0);
        cfg_writer.write();

        let ctrl = ED047TC1 {
            i8080: i8080::I8080::new(
                lcd_cam.lcd,
                channel.tx,
                tx_pins,
                10.MHz(),
                i8080::Config {
                    cd_idle_edge: false,  // dc_idle_level
                    cd_cmd_edge: true,    // dc_cmd_level
                    cd_dummy_edge: false, // dc_dummy_level
                    cd_data_edge: false,  // dc_data_level
                    ..Default::default()
                },
                &clocks,
            )
            .with_ctrl_pins(io.pins.gpio40, io.pins.gpio41),
            cfg_writer,
            rmt: rmt::Rmt::new(rmt, &clocks),
        };
        ctrl
    }

    pub(crate) fn power_on(&mut self) {
        self.cfg_writer.config.scan_direction = true;
        self.cfg_writer.config.power_disable = false;
        self.cfg_writer.write();
        busy_delay(100 * 240);
        self.cfg_writer.config.neg_power_enable = true;
        self.cfg_writer.write();
        busy_delay(500 * 240);
        self.cfg_writer.config.pos_power_enable = true;
        self.cfg_writer.write();
        busy_delay(100 * 240);
        self.cfg_writer.config.stv = true;
        self.cfg_writer.write();
    }

    // FIXME: check epdiy regarding correct poweroff sequence
    pub(crate) fn power_off(&mut self) {
        self.cfg_writer.config.pos_power_enable = false;
        self.cfg_writer.write();
        busy_delay(10 * 240);
        self.cfg_writer.config.neg_power_enable = false;
        self.cfg_writer.write();
        busy_delay(100 * 240);
        self.cfg_writer.config.power_disable = true;
        self.cfg_writer.write();
        self.cfg_writer.config.stv = false;
        self.cfg_writer.write();
    }

    pub(crate) fn frame_start(&mut self) -> crate::Result<()> {
        self.cfg_writer.config.mode = true;
        self.cfg_writer.write();

        self.rmt.pulse(10, 10, true)?;

        self.cfg_writer.config.stv = false;
        self.cfg_writer.write();
        //busy_delay(240);
        self.rmt.pulse(10000, 1000, false)?;
        self.cfg_writer.config.stv = true;
        self.cfg_writer.write();
        // self.rmt.pulse(0, 100, true)?;
        self.rmt.pulse(10, 10, true)?;
        self.rmt.pulse(10, 10, true)?;
        self.rmt.pulse(10, 10, true)?;
        self.rmt.pulse(10, 10, true)?;

        self.cfg_writer.config.output_enable = true;
        self.cfg_writer.write();
        self.rmt.pulse(10, 10, true)?;

        Ok(())
    }

    pub(crate) fn latch_row(&mut self) {
        self.cfg_writer.config.latch_enable = true;
        self.cfg_writer.write();

        self.cfg_writer.config.latch_enable = false;
        self.cfg_writer.write();
    }

    pub(crate) fn skip(&mut self) -> crate::Result<()> {
        self.rmt.pulse(45, 5, false)?;
        Ok(())
    }

    pub(crate) fn output_row(&mut self, output_time: u16) -> crate::Result<()> {
        self.latch_row();
        self.rmt.pulse(output_time, 50, false)?;
        let buf = dma_buffer();
        let tx = self
            .i8080
            .send_dma(0, 0, &buf)
            .map_err(|err| crate::Error::Dma(err))?;
        tx.wait().map_err(|err| crate::Error::Dma(err))?;

        Ok(())
    }

    pub(crate) fn frame_end(&mut self) -> crate::Result<()> {
        self.cfg_writer.config.output_enable = false;
        self.cfg_writer.write();
        self.cfg_writer.config.mode = true;
        self.cfg_writer.write();
        self.rmt.pulse(10, 10, true)?;
        self.rmt.pulse(10, 10, true)?;

        Ok(())
    }

    pub(crate) fn set_buffer(&self, data: &[u8]) {
        let buffer = dma_buffer();
        buffer[..BYTES_PER_LINE].copy_from_slice(data);
    }
}

#[inline(always)]
fn busy_delay(wait_cycles: u32) {
    let target = cycles() + wait_cycles as u64;
    while cycles() < target {}
}

#[inline(always)]
fn cycles() -> u64 {
    esp_hal::xtensa_lx::timer::get_cycle_count() as u64
}
