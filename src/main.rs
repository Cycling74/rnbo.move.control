use {
    crate::{
        controller::StateController,
        display::{DrawCommand, MoveDisplay},
        midi::Midi,
    },
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
    regex::Regex,
    reqwest_websocket::{Message, RequestBuilderExt, WebSocket},
    rosc::{OscMessage, OscPacket, OscType},
    std::{
        cmp::{Ordering, PartialEq, PartialOrd},
        collections::HashMap,
        error::Error,
        ops::{Deref, DerefMut},
        rc::Rc,
        sync::mpsc as sync_mpsc,
        thread,
        time::{Duration, Instant},
    },
    tokio::sync::mpsc as async_mpsc,
    serde::{Deserialize, Serialize}
};

//NOTE channel type should match the reciever: https://users.rust-lang.org/t/communicating-between-sync-and-async-code/41005/3

mod controller;
mod display;
mod midi;
mod param;
mod patcher;

const HTTP_QUERY_DELAY: Duration = Duration::from_millis(20);

struct Driver {
    display: Port<MidiOut>,
    midi_out: Port<MidiOut>,
    midi_in: Port<MidiIn>,
    draw_queue: sync_mpsc::Receiver<DrawCommand>,
    midi_in_queue: async_mpsc::Sender<Midi>,
    midi_out_queue: sync_mpsc::Receiver<Midi>,
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
                match i.bytes[0] {
                    0x90 | 0x80 => match i.bytes[1] {
                        //pad butttons, step buttons
                        68..=99 | 16..=31 => true,
                        _ => false,
                    },
                    0xD0 => true,
                    _ => false,
                }
            } else {
                false
            };
            if thru {
                midi_out.write(&i).unwrap();
            } else {
                //println!("not thru {:02x?}", i.bytes);
                let _ = self.midi_in_queue.try_send(Midi::new(i.bytes));
            }
        }

        for i in self.midi_out_queue.try_iter() {
            let m = RawMidi {
                time: 0,
                bytes: i.bytes(),
            };
            midi_out.write(&m).unwrap();
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
            if let (Some(a), Some(b)) = (client.port_by_id(port_id_a), client.port_by_id(port_id_b))
            {
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

async fn with_client(c: Client) -> Result<(), Box<dyn Error>> {
    let (draw_tx, draw_rx) = sync_mpsc::sync_channel(1);
    let (midi_out_tx, midi_out_rx) = sync_mpsc::sync_channel(1024);
    let (midi_in_tx, mut midi_in_rx) = async_mpsc::channel(1024);
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
        midi_in_queue: midi_in_tx,
        midi_out_queue: midi_out_rx,
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

    let mut display = Rc::new(tokio::sync::Mutex::new(display));

    let state: std::sync::Arc<tokio::sync::Mutex<StateController>> = std::sync::Arc::new(
        tokio::sync::Mutex::new(StateController::new(midi_out_tx, &mut display)),
    );

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

    let process_midi = async {
        while let Some(midi) = midi_in_rx.recv().await {
            let mut c = state.lock().await;
            c.handle_midi(midi.bytes()).await;
        }
    };

    let inst_query: tokio::sync::Mutex<Option<Instant>> = tokio::sync::Mutex::new(None);
    let sets_query: tokio::sync::Mutex<Option<Instant>> = tokio::sync::Mutex::new(None);

    let inst_path_regex = Regex::new(r"^/rnbo/inst/\d+$").expect("to create instance regex");

    //http://c74rpi.local:5678

    async fn get_instances() -> Option<HashMap<usize, PatcherInst>> {
        if let Ok(res) = reqwest::Client::new()
            .get("http://127.0.0.1:5678/rnbo/inst")
            .send()
            .await
        {
            if let Ok(res) = res.json().await {
                return PatcherInst::parse_all(&res);
            }
        }
        None
    }

    async fn get_sets() -> Option<Vec<String>> {
        if let Ok(res) = reqwest::Client::new()
            .get("http://127.0.0.1:5678/rnbo/inst/control/sets/load?RANGE")
            .send()
            .await
        {
            let res: Result<SetRange, _> = res.json().await;
            if let Ok(res) = res {
                return Some(res.range[0].vals.clone())
            }
        }
        None
    }

    let inst_query_future = async {
        loop {
            tokio::time::sleep(HTTP_QUERY_DELAY).await;
            {
                let mut g = sets_query.lock().await;
                if let Some(v) = g.deref() {
                    if Instant::now() - *v > HTTP_QUERY_DELAY {
                        if let Some(sets) = get_sets().await {
                            *g = None;
                            let mut g = state.lock().await;
                            g.set_set_names(&sets);
                        }
                    }
                }
            }

            {
                let mut g = inst_query.lock().await;
                if let Some(v) = g.deref() {
                    if Instant::now() - *v > HTTP_QUERY_DELAY {
                        //println!("got instance update");
                        if let Some(inst) = get_instances().await {
                            *g = None;
                            let mut g = state.lock().await;
                            g.set_state(inst);
                        }
                    }
                }
            }
        }
    };

    #[derive(Serialize, Deserialize, Debug)]
    struct SetRangeItem {
        #[serde(rename = "VALS")]
        vals: Vec<String>
    }

    #[derive(Serialize, Deserialize, Debug)]
    struct SetRange {
        #[serde(rename = "RANGE")]
        range: [SetRangeItem; 1]
    }

    let web_future = async {
        loop {
            if let Ok(_res) = reqwest::Client::new()
                .get("http://127.0.0.1:5678/rnbo")
                .send()
                .await
            {
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
                            let mut g = state.lock().await;
                            g.set_ws(tx);
                        }

                        //do inst query
                        {
                            let mut g = inst_query.lock().await;
                            *g = Some(Instant::now());
                        }

                        //do sets query
                        {
                            let mut g = sets_query.lock().await;
                            *g = Some(Instant::now());
                        }

                        while let Ok(message) = rx.try_next().await {
                            if let Some(message) = message {
                                match message {
                                    Message::Text(text) => {
                                        let cmd: serde_json::Result<serde_json::Value> =
                                            serde_json::from_str(text.as_str());
                                        if let Ok(cmd) = cmd {
                                            if let (Some(name), Some(data)) = (
                                                cmd.get("COMMAND").unwrap().as_str(),
                                                cmd.get("DATA"),
                                            ) {
                                                if let Some(path) = match name {
                                                    "ATTRIBUTES_CHANGED" => {
                                                        match data
                                                            .get("FULL_PATH")
                                                            .map(|p| p.as_str())
                                                            .flatten()
                                                        {
                                                            Some(
                                                                "/rnbo/inst/control/sets/load",
                                                            ) => {
                                                                let range: Result<SetRange, _> = serde_json::from_value(data.clone());
                                                                if let Ok(range) = range {
                                                                    let mut g = state.lock().await;
                                                                    g.set_set_names(&range.range[0].vals);
                                                                }
                                                            }
                                                            _ => (),
                                                        }
                                                        //println!("data {:?}", cmd);
                                                        None
                                                        /*
                                                        data
                                                        .get("FULL_PATH")
                                                        .map(|p| p.as_str())
                                                        .flatten(),
                                                        */
                                                    }
                                                    "PATH_ADDED" | "PATH_REMOVED" => data.as_str(),
                                                    _ => None,
                                                } {
                                                    //added or removed
                                                    if inst_path_regex.is_match(path) {
                                                        let mut g = inst_query.lock().await;
                                                        *g = Some(Instant::now());
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Message::Binary(vec) => {
                                        match rosc::decoder::decode_udp(vec.as_slice()) {
                                            Ok((_, OscPacket::Message(m))) => {
                                                let mut g = state.lock().await;
                                                g.handle_osc(&m).await;
                                            }
                                            _ => (),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };

    let disconnect_future = async {
        while let Some((a, b)) = disconnect_rx.recv().await {
            let client = c.as_client();
            if let (Some(a), Some(b)) = (client.port_by_id(a), client.port_by_id(b)) {
                let _ = client.disconnect_ports(&a, &b);
            }
        }
    };

    let signal_future = async {
        tokio::signal::ctrl_c().await.unwrap();
    };
    tokio::select! {
        _ = display_future => (), _ = web_future => (),  _ = inst_query_future => (),
        _ = signal_future => (), _ = process_midi => (), _ = disconnect_future => (),
    };
    let _ = c.deactivate();
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let name = "move-control";
    let pollms = 500;

    //wait until jack exists
    loop {
        if let Ok((c, _status)) = Client::new(name, ClientOptions::empty()) {
            let res = with_client(c).await;
            match res {
                Ok(()) => break,
                Err(e) => {
                    println!("error {:?}", e);
                    //add a little extra time if there is an error
                    tokio::time::sleep(Duration::from_millis(pollms)).await;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(pollms)).await;
    }
    Ok(())
}
