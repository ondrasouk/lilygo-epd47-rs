use core::ops::DerefMut;

use esp_hal::clock::Clocks;
use esp_hal::gpio::GpioPin;
use esp_hal::peripheral::{Peripheral, PeripheralRef};
use esp_hal::prelude::*;
use esp_hal::rmt::{Channel, PulseCode, TxChannel, TxChannelCreator};
use esp_hal::{into_ref, peripherals, rmt, Blocking};

pub(crate) struct Rmt<'a> {
    tx_channel: Option<Channel<Blocking, 1>>,
    clocks: &'a Clocks<'a>,
    rmt: PeripheralRef<'a, peripherals::RMT>,
}

impl<'a> Rmt<'a> {
    pub(crate) fn new(rmt: impl Peripheral<P = peripherals::RMT> + 'a, clocks: &'a Clocks) -> Self {
        into_ref!(rmt);
        Rmt {
            tx_channel: None,
            clocks,
            rmt,
        }
    }

    fn ensure_channel(&mut self) -> Result<(), crate::Error> {
        if self.tx_channel.is_some() {
            return Ok(());
        }
        let rmt = rmt::Rmt::new(
            unsafe { self.rmt.deref_mut().clone_unchecked() }, // TODO: find better solution
            80.MHz(),
            self.clocks,
            None,
        )
        .map_err(|err| crate::Error::Rmt(err))?;
        let tx_channel = rmt
            .channel1
            .configure(
                unsafe { GpioPin::<38>::steal() }, // TODO: find better solution
                rmt::TxChannelConfig {
                    clk_divider: 8,
                    idle_output_level: false,
                    idle_output: true,
                    carrier_modulation: false,
                    carrier_level: false,
                    ..Default::default()
                },
            )
            .map_err(|err| crate::Error::Rmt(err))?;
        self.tx_channel = Some(tx_channel);
        Ok(())
    }

    pub(crate) fn pulse(&mut self, high: u16, low: u16, wait: bool) -> Result<(), crate::Error> {
        self.ensure_channel()?;
        let tx_channel = self.tx_channel.take().ok_or(crate::Error::Unknown)?;
        let data = if high > 0 {
            [
                PulseCode {
                    level1: true,
                    length1: high,
                    level2: false,
                    length2: low,
                },
                PulseCode::default(), // end of pulse indicator
            ]
        } else {
            [
                PulseCode {
                    level1: true,
                    length1: low,
                    level2: false,
                    length2: 0,
                },
                // FIXME: find more elegant solution
                PulseCode::default(), // end of pulse indicator (redundant, but simplifies the code)
            ]
        };
        let tx = tx_channel.transmit(&data);
        // FIXME: This is the culprit.. We need the channel later again but can't wait due to some
        // time sensitive operations. Not sure how to solve this
        if wait {
            self.tx_channel = Some(
                tx.wait()
                    .map_err(|(err, _)| err)
                    .map_err(|err| crate::Error::Rmt(err))?,
            );
        }
        Ok(())
    }
}
