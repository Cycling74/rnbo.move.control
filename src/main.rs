use {
    crate::display::MoveDisplay,
    embedded_graphics::{
        pixelcolor::BinaryColor,
        prelude::*,
        primitives::{Circle, PrimitiveStyle},
    },
    jack::{
        Client, ClientOptions, Control, MidiIn, MidiOut, Port, PortId, ProcessScope, RawMidi, Time,
        Unowned,
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

struct ConnectionControl {
    display_port: Port<Unowned>,
    system_display_port: Port<Unowned>,

    midi_in_port: Port<Unowned>,
    system_midi_out_port: Port<Unowned>,
}

impl jack::NotificationHandler for ConnectionControl {
    fn ports_connected(
        &mut self,
        client: &Client,
        port_id_a: PortId,
        port_id_b: PortId,
        are_connected: bool,
    ) {
        //don't allow anything to connect to system display, system midi port, our midi in port or our display port except us
        if are_connected {
            if let Some(a) = client.port_by_id(port_id_a) {
                if let Some(b) = client.port_by_id(port_id_b) {
                    if (a != self.display_port && b == self.system_display_port)
                        || (a == self.display_port && b != self.system_display_port)
                        || (a == self.system_midi_out_port && b != self.midi_in_port)
                        || (a != self.system_midi_out_port && b == self.midi_in_port)
                    {
                        let _ = client.disconnect_ports(&a, &b);
                    }
                }
            }
        }
    }
}

fn main() {
    let name = "move-control";
    let (c, _status) = Client::new(name, ClientOptions::empty()).expect("error creating client");

    let run = Arc::new(AtomicBool::new(true));

    let r = run.clone();
    ctrlc::set_handler(move || {
        r.store(false, Ordering::Relaxed);
    })
    .expect("Error setting Ctrl-C handler");

    let display = c
        .register_port("display", MidiOut)
        .expect("error creating display port");
    let midi_out = c
        .register_port("midi_out", MidiOut)
        .expect("error creating midi_out");
    let midi_in = c
        .register_port("midi_in", MidiIn)
        .expect("error creating midi_in");

    let system_display_port = c
        .port_by_name("system:display")
        .expect("error getting system:display");
    let system_midi_out_port = c
        .port_by_name("system:midi_capture")
        .expect("error getting system:midi_capture");

    let mut move_display = MoveDisplay::new();
    let control = ConnectionControl {
        display_port: display.clone_unowned(),
        system_display_port,
        midi_in_port: midi_in.clone_unowned(),
        system_midi_out_port,
    };

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

    let c = c
        .activate_async(control, driver)
        .expect("error activating client");
    {
        let c = c.as_client();
        let name = c.name().to_string();

        //sleep for a bit to let the clients setup connections
        thread::sleep(Duration::from_millis(200));

        //disconnect ports that might have been automatically connected
        let display_name = format!("{}:display", name);
        let midi_in_name = format!("{}:midi_in", name);
        for (name, is_source) in [
            (display_name.clone(), true),
            (midi_in_name.clone(), false),
            ("system:midi_capture".to_string(), true),
            ("system:display".to_string(), false),
        ] {
            let p = c
                .port_by_name(name.as_str())
                .expect(format!("to get {} port", name).as_str());
            for n in p.get_connections() {
                let _ = if is_source {
                    c.disconnect_ports_by_name(name.as_str(), n.as_str())
                } else {
                    c.disconnect_ports_by_name(n.as_str(), name.as_str())
                };
            }
        }

        //connect what we want to be connected
        c.connect_ports_by_name(display_name.as_str(), "system:display")
            .unwrap();
        c.connect_ports_by_name(
            format!("{}:midi_out", name).as_str(),
            "system:midi_playback",
        )
        .unwrap();
        c.connect_ports_by_name("system:midi_capture", format!("{}:midi_in", name).as_str())
            .unwrap();
    }

    while run.load(Ordering::Relaxed) {
        thread::sleep(Duration::from_millis(100));
    }
    let _ = c.deactivate();
}
