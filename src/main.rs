use {
    crate::display::MoveDisplay,
    embedded_graphics::{
        pixelcolor::BinaryColor,
        prelude::*,
        primitives::{Circle, PrimitiveStyle},
    },
    jack::{
        Client, ClientOptions, Control, MidiIn, MidiOut, Port, PortSpec, ProcessScope, RawMidi,
        Time,
    },
    std::{
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        thread,
        time::Duration,
    },
};

mod display;

struct Driver {
    move_display: MoveDisplay,
    display: Port<MidiOut>,
    midi_out: Port<MidiOut>,
    midi_in: Port<MidiIn>,
    last: Time,
    period: Time,
}

//display rate: 22.928ms

impl jack::ProcessHandler for Driver {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        let times = ps.cycle_times().unwrap();

        if times.current_usecs - self.last > self.period {
            self.last = times.current_usecs;
            self.move_display.draw_if(|bytes| {
                let m = RawMidi { time: 0, bytes };
                let mut w = self.display.writer(ps);
                w.write(&m).unwrap();
            });
        }

        let midi_in = self.midi_in.iter(ps);
        let mut midi_out = self.midi_out.writer(ps);
        for i in midi_in {
            //TODO filter
            midi_out.write(&i).unwrap();
        }

        Control::Continue
    }
}

fn main() {
    let name = "move-control";
    let (c, _status) = Client::new(name, ClientOptions::empty()).unwrap();

    let run = Arc::new(AtomicBool::new(true));

    let r = run.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::Relaxed);
    })
    .expect("Error setting Ctrl-C handler");

    let display = c.register_port("display", MidiOut).unwrap();
    let midi_out = c.register_port("midi_out", MidiOut).unwrap();
    let midi_in = c.register_port("midi_in", MidiIn).unwrap();

    let mut move_display = MoveDisplay::new();

    let circle = Circle::new(Point::new(22, 22), 20)
        .into_styled(PrimitiveStyle::with_stroke(BinaryColor::On, 1));
    circle.draw(&mut move_display).unwrap();

    let driver = Driver {
        move_display,
        display,
        midi_out,
        midi_in,
        last: 0,
        period: 22_928,
    };

    let name = c.name().to_string();
    let c = c.activate_async((), driver).unwrap();
    {
        let c = c.as_client();
        c.connect_ports_by_name(format!("{}:display", name).as_str(), "system:display")
            .unwrap();
        c.connect_ports_by_name(
            format!("{}:midi_out", name).as_str(),
            "system:midi_playback",
        )
        .unwrap();
        c.connect_ports_by_name("system:midi_capture", format!("{}:midi_in", name).as_str())
            .unwrap();
    }

    println!("Running");
    while run.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(100));
    }
    c.deactivate().unwrap();
}
