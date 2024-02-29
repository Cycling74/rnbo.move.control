use {
    crate::display::MoveDisplay,
    embedded_graphics::{
        mono_font::MonoTextStyle,
        pixelcolor::BinaryColor,
        prelude::*,
        text::{Alignment, Text},
    },
    futures_util::{SinkExt, StreamExt, TryStreamExt},
    jack::{
        Client, ClientOptions, Control, MidiIn, MidiOut, Port, PortId, ProcessScope, RawMidi,
        Unowned,
    },
    param::Param,
    reqwest_websocket::{Message, RequestBuilderExt, WebSocket},
    rosc::OscPacket,
    std::{error::Error, ops::DerefMut, sync::mpsc as sync_mpsc, thread, time::Duration},
    tokio::sync::mpsc as async_mpsc,
};

//NOTE channel type should match the reciever: https://users.rust-lang.org/t/communicating-between-sync-and-async-code/41005/3

mod display;
mod param;

struct DrawCommand {
    pub data: [u8; display::BUFFER_LEN],
}
struct Midi {
    bytes: [u8; 3],
}

impl Midi {
    pub fn new(v: &[u8]) -> Self {
        let mut bytes = [0; 3];
        bytes.copy_from_slice(v);
        Self { bytes }
    }

    pub fn bytes(&self) -> &[u8; 3] {
        &self.bytes
    }
}

struct Driver {
    display: Port<MidiOut>,
    midi_out: Port<MidiOut>,
    midi_in: Port<MidiIn>,
    draw_queue: sync_mpsc::Receiver<DrawCommand>,
    midi_queue: async_mpsc::Sender<Midi>,
}

//display rate: 22.928ms

impl jack::ProcessHandler for Driver {
    fn process(&mut self, _: &Client, ps: &ProcessScope) -> Control {
        if let Ok(cmd) = self.draw_queue.try_recv() {
            let m = RawMidi {
                time: 0,
                bytes: &cmd.data,
            };
            let mut w = self.display.writer(ps);
            w.write(&m).unwrap();
        }

        let midi_in = self.midi_in.iter(ps);
        let mut midi_out = self.midi_out.writer(ps);
        for i in midi_in {
            //only send pad buttons and step buttons thru
            let thru = if i.bytes.len() == 3 {
                let thru = match i.bytes[0] {
                    0x90 | 0x80 => match i.bytes[1] {
                        //pad butttons, step buttons
                        68..=99 | 16..=31 => true,
                        _ => false,
                    },
                    _ => false,
                };
                if !thru {
                    let _ = self.midi_queue.try_send(Midi::new(i.bytes));
                }
                thru
            } else {
                false
            };
            if thru {
                midi_out.write(&i).unwrap();
            }
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let name = "move-control";
    let (c, _status) = Client::new(name, ClientOptions::empty()).expect("error creating client");

    let display_port = c
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

    let mut display = MoveDisplay::new();
    let control = ConnectionControl {
        display_port: display_port.clone_unowned(),
        system_display_port,
        midi_in_port: midi_in.clone_unowned(),
        system_midi_out_port,
    };

    let style = MonoTextStyle::new(&profont::PROFONT_24_POINT, BinaryColor::On);
    let size = display.size();
    Text::with_alignment(
        "RNBO\non Move",
        Point::new(size.width as i32 / 2, size.height as i32 / 2),
        style,
        Alignment::Center,
    )
    .draw(&mut display)?;

    let (draw_tx, draw_rx) = sync_mpsc::sync_channel(1);
    let (midi_tx, midi_rx) = async_mpsc::channel(1024);

    let driver = Driver {
        display: display_port,
        midi_out,
        midi_in,
        draw_queue: draw_rx,
        midi_queue: midi_tx,
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

    let display = tokio::sync::Mutex::new(display);
    let display_future = async {
        loop {
            //frame rate
            tokio::time::sleep(Duration::from_millis(23)).await;
            let mut display = display.lock().await;
            display.draw_if(|data| {
                draw_tx.send(DrawCommand { data: data.clone() }).unwrap();
            });
        }
    };

    let web_future = async {
        let mut ws: Option<WebSocket> = None;
        let mut error = false;
        loop {
            if let Some(ws) = &mut ws {
                if let Ok(message) = ws.try_next().await {
                    if let Some(message) = message {
                        match message {
                            Message::Text(text) => {
                                println!("received: {text}")
                            }
                            Message::Binary(vec) => {
                                let osc = rosc::decoder::decode_udp(vec.as_slice());
                                if let Ok((_, p)) = osc {
                                    match p {
                                        OscPacket::Message(m) => {
                                            let style = MonoTextStyle::new(
                                                &profont::PROFONT_7_POINT,
                                                BinaryColor::On,
                                            );
                                            let mut display = display.lock().await;
                                            display.clear(BinaryColor::Off).unwrap();
                                            let size = display.size();
                                            Text::with_alignment(
                                                m.addr.as_str(),
                                                Point::new(
                                                    size.width as i32 / 2,
                                                    size.height as i32 / 2,
                                                ),
                                                style,
                                                Alignment::Center,
                                            )
                                            .draw(display.deref_mut())
                                            .unwrap();
                                        }
                                        _ => (),
                                    }
                                }
                            }
                        }
                    }
                } else {
                    error = true;
                }
            } else {
                if let Ok(res) = reqwest::Client::new()
                    .get("http://127.0.0.1:5678/rnbo/inst/0/params")
                    .send()
                    .await
                {
                    let res: serde_json::Value = res.json().await.unwrap();
                    let params = Param::parse_all(&res);
                    println!("got params {:?}", params);
                } else {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }

                if let Ok(res) = reqwest::Client::new()
                    .get("http://127.0.0.1:5678")
                    .upgrade()
                    .send()
                    .await
                {
                    if let Ok(websocket) = res.into_websocket().await {
                        ws = Some(websocket);
                    }
                } else {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
            if error {
                error = false;
                ws = None;
            }
        }
    };

    let signal_future = async {
        tokio::signal::ctrl_c().await.unwrap();
    };
    tokio::select! {
        _ = display_future => (), _ = web_future => (), _ = signal_future => ()
    };
    let _ = c.deactivate();
    Ok(())
}
