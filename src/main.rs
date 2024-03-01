use {
    crate::display::MoveDisplay,
    embedded_graphics::{
        mono_font::MonoTextStyle,
        pixelcolor::BinaryColor,
        prelude::*,
        text::{Alignment, Text},
    },
    futures_util::{stream::SplitSink, SinkExt, StreamExt, TryStreamExt},
    jack::{
        Client, ClientOptions, Control, MidiIn, MidiOut, Port, PortId, ProcessScope, RawMidi,
        Unowned,
    },
    param::Param,
    patcher::PatcherInst,
    reqwest_websocket::{Message, RequestBuilderExt, WebSocket},
    rosc::{OscMessage, OscPacket, OscType},
    std::{
        collections::HashMap,
        error::Error,
        ops::{Deref, DerefMut},
        sync::mpsc as sync_mpsc,
        thread,
        time::Duration,
    },
    tokio::sync::mpsc as async_mpsc,
};

//NOTE channel type should match the reciever: https://users.rust-lang.org/t/communicating-between-sync-and-async-code/41005/3

mod display;
mod param;
mod patcher;

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

    disconnect_queue: async_mpsc::Sender<(PortId, PortId)>,
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
                        let _ = self.disconnect_queue.try_send((port_id_a, port_id_b));
                    }
                }
            }
        }
    }
}

struct State {
    pub instances: HashMap<usize, PatcherInst>,
    pub params: HashMap<String, usize>,
    pub selected_param: Option<(usize, usize)>,
}

impl State {
    pub fn new(instances: HashMap<usize, PatcherInst>) -> Self {
        let mut params: HashMap<String, usize> = HashMap::new();
        for (index, v) in instances.iter() {
            for p in v.params().iter() {
                params.insert(p.addr().to_string(), *index);
            }
        }
        Self {
            instances,
            params,
            selected_param: Some((0, 0)),
        }
    }

    fn handle_osc(&mut self, msg: &OscMessage) -> Option<(usize, usize)> {
        if msg.args.len() == 1 {
            if let Some(index) = self.params.get(&msg.addr) {
                if let Some(inst) = self.instances.get_mut(&index) {
                    let pindex = match &msg.args[0] {
                        OscType::Double(v) => inst.update_param_f64(&msg.addr, *v),
                        OscType::Float(v) => inst.update_param_f64(&msg.addr, *v as f64),
                        OscType::Int(v) => inst.update_param_f64(&msg.addr, *v as f64),
                        OscType::String(v) => inst.update_param_s(&msg.addr, v),
                        _ => return None,
                    }?;
                    return Some((*index, pindex));
                }
            }
        }
        None
    }

    fn render_osc(&mut self, inst: usize, param: usize, v: isize) -> Option<OscMessage> {
        if let Some(inst) = self.instances.get_mut(&inst) {
            if let Some(param) = inst.params_mut().get_mut(param) {
                let mut args = Vec::new();
                let step = 0.01;
                //operate on the normalized value.. TODO, change step
                let v = (param.norm() + if v < 64 { step } else { -step }).clamp(0.0, 1.0);
                param.set_norm(v); //TODO get norm from OSC
                args.push(OscType::Double(v));
                return Some(OscMessage {
                    addr: format!("{}/normalized", param.addr()),
                    args,
                });
            }
        }
        None
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            instances: HashMap::new(),
            params: HashMap::new(),
            selected_param: None,
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let name = "move-control";
    let (c, _status) = Client::new(name, ClientOptions::empty()).expect("error creating client");

    let (draw_tx, draw_rx) = sync_mpsc::sync_channel(1);
    let (midi_tx, mut midi_rx) = async_mpsc::channel(1024);
    let (disconnect_tx, mut disconnect_rx) = async_mpsc::channel(128);

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
        disconnect_queue: disconnect_tx,
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

    let render_selected = |state: &State, display: &mut MoveDisplay| {
        let style = MonoTextStyle::new(&profont::PROFONT_12_POINT, BinaryColor::On);
        display.clear(BinaryColor::Off).unwrap();
        let size = display.size();

        if let Some((inst, param)) = state.selected_param {
            if let Some(inst) = state.instances.get(&inst) {
                if let Some(param) = inst.params().get(param) {
                    let s = format!("{}\n{}", param.name(), param.render_value());
                    Text::with_alignment(
                        s.as_str(),
                        Point::new(size.width as i32 / 2, size.height as i32 / 2),
                        style,
                        Alignment::Center,
                    )
                    .draw(display)
                    .unwrap();
                }
            }
        }
    };

    let state: tokio::sync::Mutex<State> = tokio::sync::Mutex::new(State::default());
    let ws_tx: tokio::sync::Mutex<Option<SplitSink<WebSocket, Message>>> =
        tokio::sync::Mutex::new(None);

    let process_midi = async {
        loop {
            if let Some(midi) = midi_rx.recv().await {
                match midi.bytes[0] {
                    0x90 => {
                        //param select
                        if midi.bytes[1] <= 8 {
                            //select!
                            let mut g = state.lock().await;
                            let mut d = display.lock().await;
                            g.selected_param = Some((0, midi.bytes[1] as usize));
                            render_selected(g.deref(), d.deref_mut());
                        }
                    }
                    0xB0 => {
                        match midi.bytes[1] {
                            //param encoders
                            index @ 71..=78 => {
                                let inst = 0;
                                let index = (index - 71) as usize;
                                let v = midi.bytes[2];
                                //left == 127
                                //right == 1
                                let mut s = state.lock().await;
                                if let Some(msg) = s.render_osc(inst, index, v as isize) {
                                    let packet = OscPacket::Message(msg);
                                    if let Ok(msg) = rosc::encoder::encode(&packet) {
                                        let mut tx = ws_tx.lock().await;
                                        if let Some(tx) = tx.deref_mut() {
                                            let _ = tx.send(Message::Binary(msg)).await;
                                        }
                                    }
                                }
                            }
                            _ => (),
                        }
                    }
                    _ => (),
                }
            }
        }
    };

    let web_future = async {
        loop {
            if let Ok(res) = reqwest::Client::new()
                .get("http://127.0.0.1:5678/rnbo/inst")
                .send()
                .await
            {
                let res: serde_json::Value = res.json().await.unwrap();
                let p = PatcherInst::parse_all(&res);
                println!("got patchers {:?}", p);
                if let Some(p) = p {
                    let mut g = state.lock().await;
                    let mut new_state = State::new(p);
                    std::mem::swap(g.deref_mut(), &mut new_state);
                }
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
                    println!("got websocket");
                    let (tx, mut rx) = websocket.split();

                    {
                        //set up sender
                        let mut g = ws_tx.lock().await;
                        *g = Some(tx);
                    }

                    loop {
                        if let Ok(message) = rx.try_next().await {
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
                                                    let mut g = state.lock().await;
                                                    let mut d = display.lock().await;
                                                    if let Some(cur) = g.handle_osc(&m) {
                                                        if let Some(sel) = g.selected_param {
                                                            if cur == sel {
                                                                render_selected(
                                                                    g.deref(),
                                                                    d.deref_mut(),
                                                                );
                                                            }
                                                        }
                                                    }
                                                }
                                                _ => (),
                                            }
                                        }
                                    }
                                }
                            }
                        } else {
                            break;
                        }
                    }
                }
            } else {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    };

    let disconnect_future = async {
        while let Some((a, b)) = disconnect_rx.recv().await {
            let client = c.as_client();
            if let Some(a) = client.port_by_id(a) {
                if let Some(b) = client.port_by_id(b) {
                    let _ = client.disconnect_ports(&a, &b);
                }
            }
        }
    };

    let signal_future = async {
        tokio::signal::ctrl_c().await.unwrap();
    };
    tokio::select! {
        _ = display_future => (), _ = web_future => (), _ = signal_future => (), _ = process_midi => (), _ = disconnect_future => (),
    };
    let _ = c.deactivate();
    Ok(())
}
