#![no_main]
#![no_std]

// Built in dependencies
use core::fmt::Write;

// Crates.io dependencies
use dw1000::{DW1000 as DW};
use dwm1001::{
    self,
    nrf52832_hal::{
        delay::Delay,
        prelude::*,
        timer::Timer,
        gpio::{Pin, Output, PushPull, Level, p0::P0_17},
        rng::Rng,
        spim::{Spim},
        nrf52832_pac::{
            TIMER0,
            SPIM2,
        },
        uarte::Baudrate as UartBaudrate,
    },
    new_dw1000,
    new_usb_uarte,
    UsbUarteConfig,
    DW_RST,
    block_timeout,
    dw1000::{
        macros::TimeoutError,
        mac::Address,
        Message,
    },
};
use heapless::{String, consts::*};
use rtfm::app;
use postcard::from_bytes;

// NOTE: Panic Provider
use panic_ramdump as _;

// Workspace dependencies
use protocol::{
    ModemUartMessages,
    CellCommand,
    RadioMessages,
};
use nrf52_bin_logger::Logger;


#[app(device = dwm1001::nrf52832_hal::nrf52832_pac)]
const APP: () = {
    static mut LED_RED_1: Pin<Output<PushPull>>     = ();
    static mut TIMER:     Timer<TIMER0>             = ();
    static mut LOGGER:    Logger<U1024, ModemUartMessages> = ();
    static mut DW1000:    DW<
                            Spim<SPIM2>,
                            P0_17<Output<PushPull>>,
                            dw1000::Ready,
                          > = ();
    static mut DW_RST_PIN: DW_RST                   = ();
    static mut RANDOM:     Rng                      = ();

    #[init]
    fn init() {
        let timer = device.TIMER0.constrain();
        let pins = device.P0.split();

        let mut uc = UsbUarteConfig::default();
        uc.baudrate = UartBaudrate::BAUD230400;

        let uarte0 = new_usb_uarte(
            device.UARTE0,
            pins.p0_05,
            pins.p0_11,
            uc,
        );

        let rng = device.RNG.constrain();

        let dw1000 = new_dw1000(
            device.SPIM2,
            pins.p0_16,
            pins.p0_20,
            pins.p0_18,
            pins.p0_17,
        );

        let mut rst_pin = DW_RST::new(pins.p0_24.into_floating_input());

        let clocks = device.CLOCK.constrain().freeze();

        let mut delay = Delay::new(core.SYST, clocks);

        rst_pin.reset_dw1000(&mut delay);
        let mut dw1000 = dw1000.init().unwrap();
        dw1000.set_address(
            Address {
                pan_id:     MODEM_PAN,
                short_addr: MODEM_ADDR,
            }
        ).unwrap();

        RANDOM = rng;
        DW_RST_PIN = rst_pin;
        DW1000 = dw1000;
        LOGGER = Logger::new(uarte0);
        TIMER = timer;
        LED_RED_1 = pins.p0_14.degrade().into_push_pull_output(Level::High);
    }

    #[idle(resources = [TIMER, LED_RED_1, LOGGER, RANDOM, DW1000])]
    fn idle() -> ! {
        let mut buffer = [0u8; 1024];
        let mut strbuf: String<U1024> = String::new();

        loop {
            let mut rx = if let Ok(rx) = resources.DW1000.receive() {
                rx
            } else {
                resources.LOGGER.warn("Failed to start receive!").unwrap();
                resources.TIMER.delay(250_000);
                continue;
            };

            resources.TIMER.start(1_000_000u32);

            match block_timeout!(&mut *resources.TIMER, rx.wait(&mut buffer)) {
                Ok(message) => {
                    if let Ok(resp) = process_message(
                        resources.LOGGER,
                        &message
                    ) {
                        resources.LOGGER.data(resp).unwrap();
                    } else {
                        strbuf.clear();
                        write!(&mut strbuf, "^ Bad message from src 0x{:04X}", message.frame.header.source.short_addr).unwrap();
                        resources.LOGGER.warn(strbuf.as_str()).unwrap();
                    }
                },
                Err(TimeoutError::Timeout) => {
                    resources.LOGGER.log("RX Timeout").unwrap();
                    continue;
                }
                Err(TimeoutError::Other(error)) => {
                    strbuf.clear();
                    write!(&mut strbuf, "RX: {:?}", error).unwrap();
                    resources.LOGGER.error(strbuf.as_str()).unwrap();
                    continue;
                }
            };
        }
    }
};

const MODEM_PAN: u16 = 0x0386;
const MODEM_ADDR: u16 = 0x0808;
const BROADCAST: u16 = 0xFFFF;

fn process_message(logger: &mut Logger<U1024, ModemUartMessages>, msg: &Message) -> Result<ModemUartMessages, ()> {
    if msg.frame.header.source.pan_id == BROADCAST {
        logger.error("bad bdcst pan!").unwrap();
        return Err(())
    }

    if msg.frame.header.source.short_addr == BROADCAST {
        logger.error("bad bdcst addr!").unwrap();
        return Err(())
    }

    if msg.frame.header.destination.pan_id != msg.frame.header.source.pan_id {
        logger.error("mismatch pan!").unwrap();
        return Err(())
    }

    if msg.frame.header.destination.short_addr != MODEM_ADDR {
        logger.error("that ain't me").unwrap();
        return Err(())
    }

    if let Ok(pmsg) = from_bytes::<RadioMessages>(msg.frame.payload) {
        match pmsg {
            RadioMessages::SetCell(sc) => {
                return Ok(ModemUartMessages::SetCell(CellCommand {
                    source: msg.frame.header.source.short_addr,
                    dest: msg.frame.header.destination.short_addr,
                    cell: sc
                }));
            }
        }
    } else {
        logger.warn("Failed to decode!").unwrap();
    }

    Err(())
}

use nb::{
    block,
};


pub fn delay<T>(timer: &mut Timer<T>, cycles: u32) where T: TimerExt {
    timer.start(cycles);
    block!(timer.wait()).expect("wait fail");
}
