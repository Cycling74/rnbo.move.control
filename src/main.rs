use {
    crate::{
        config::*,
        controller::{Caps, StateController},
        display::{DrawCommand, MoveDisplay},
        midi::Midi,
        view::ParamView,
    },
    clap::Parser,
    embedded_graphics::{
        mono_font::MonoTextStyle,
        pixelcolor::BinaryColor,
        prelude::*,
        text::{Alignment, Text},
    },
    futures_util::{StreamExt, TryStreamExt},
    jack::{
        AudioIn, AudioOut, Client, ClientOptions, Control, MidiIn, MidiOut, Port, PortId,
        ProcessScope, RawMidi, Unowned,
    },
    patcher::PatcherInst,
    regex::Regex,
    reqwest_websocket::{Message, RequestBuilderExt},
    rlimit::setrlimit,
    rosc::OscPacket,
    serde::{Deserialize, Serialize},
    std::process::Stdio,
    std::{
        collections::HashMap,
        error::Error,
        ops::Deref,
        path::PathBuf,
        rc::Rc,
        sync::{
            atomic::{AtomicU8, Ordering},
            mpsc as sync_mpsc, Arc,
        },
        thread,
        time::{Duration, Instant},
    },
    tokio::{
        io::{AsyncBufReadExt, BufReader},
        sync::mpsc as async_mpsc,
    },
};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// path to configuration json
    #[arg(short, long, default_value = "/data/UserData/rnbo/config/control.json")]
    config: String,

    /// path to startup config json
    #[arg(
        short,
        long,
        default_value = "/data/UserData/rnbo/config/control-startup.json"
    )]
    startup: Option<String>,
}

//NOTE channel type should match the reciever: https://users.rust-lang.org/t/communicating-between-sync-and-async-code/41005/3

mod config;
mod controller;
mod display;
mod font;
mod midi;
mod param;
mod patcher;
mod view;
mod widget;

const HTTP_QUERY_DELAY: Duration = Duration::from_millis(200);
const HTTP_INITIAL_QUERY_DELAY: Duration = Duration::from_millis(500);

const POLL_CHILD_STATUS_MS: u64 = 2000;

struct Driver {
    display: Port<MidiOut>,
    midi_thru: Port<MidiOut>,
    midi_control_out: Port<MidiOut>,
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
        let mut midi_thru = self.midi_thru.writer(ps);
        for i in midi_in {
            //println!("got midi: {:?}", i);
            //only filter out sysex, jog wheel, volume, back and menu
            //optionally (TODO) filter out encoders
            let filter = match i.bytes.len() {
                3 => match i.bytes[0] {
                    //Note on/off
                    0x9F | 0x8F => match i.bytes[1] {
                        //encoders, volume, wheel
                        0..=9 => true,
                        _ => false,
                    },
                    //CC
                    0xBF => match i.bytes[1] {
                        //wheel press/turn, back, play/stop, menu (session/note)
                        3 | 14 | 51 | 85 | 50 => true,
                        //encoders.., volume
                        71..=79 => true,
                        _ => false,
                    },
                    0xF0 | 0xF7 => true,         //sysex start or end
                    _ => i.bytes[0] & 0x80 == 0, //sysex continue
                },
                2 => i.bytes[0] == 0xF7 || i.bytes[1] == 0xF7,
                1 => i.bytes[0] == 0xF7,
                _ => {
                    println!("unhandled {} byte MIDI message", i.bytes.len());
                    continue;
                }
            };

            if filter {
                //println!("not thru {:02x?}", i.bytes);
                let _ = self.midi_in_queue.try_send(Midi::new(i.bytes));
            } else {
                midi_thru.write(&i).unwrap();
            }
        }

        let mut midi_out = self.midi_control_out.writer(ps);
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

fn port_uuid<PS: jack::PortSpec + Send>(port: &Port<PS>) -> Option<jack::jack_sys::jack_uuid_t> {
    unsafe {
        let uuid = jack::jack_sys::jack_port_uuid(port.raw());
        if jack::jack_sys::jack_uuid_empty(uuid) == 0 {
            Some(uuid)
        } else {
            None
        }
    }
}

fn port_set_group<PS: jack::PortSpec + Send>(
    client: &Client,
    port: &Port<PS>,
    property: &jack::Property,
) {
    const PORTGROUP: &str = "http://jackaudio.org/metadata/port-group";
    if let Some(uuid) = port_uuid(port) {
        let _ = client.property_set(uuid, PORTGROUP, property);
    }
}

fn port_set_pretty<PS: jack::PortSpec + Send>(
    client: &Client,
    port: &Port<PS>,
    property: &jack::Property,
) {
    const PORTGROUP: &str = "http://jackaudio.org/metadata/pretty-name";
    if let Some(uuid) = port_uuid(port) {
        let _ = client.property_set(uuid, PORTGROUP, property);
    }
}

async fn with_client(
    c: Client,
    logger: &mut syslog::Logger<syslog::LoggerBackend, syslog::Formatter3164>,
    startup: &Vec<StartupProcess>,
    config: &PathBuf,
    caps: Caps,
) -> Result<(), Box<dyn Error>> {
    let (draw_tx, draw_rx) = sync_mpsc::sync_channel(1);
    let (midi_out_tx, midi_out_rx) = sync_mpsc::sync_channel(1024);
    let (midi_in_tx, mut midi_in_rx) = async_mpsc::channel(1024);
    let (disconnect_tx, mut disconnect_rx) = async_mpsc::channel(128);

    let display_port = c
        .register_port("display", MidiOut)
        .expect("error creating display port");
    let midi_thru = c
        .register_port("midi_out", MidiOut)
        .expect("error creating midi_out");
    let midi_control_out = c
        .register_port("midi_control_out", MidiOut)
        .expect("error creating midi_control_out");
    let midi_in = c
        .register_port("midi_in", MidiIn)
        .expect("error creating midi_in");

    let system_display_port = c
        .port_by_name("system:display")
        .expect("error getting system:display");
    let system_midi_capture_port = c
        .port_by_name("system:midi_capture")
        .expect("error getting system:midi_capture");

    //add properties

    let hidden = jack::Property::new("rnbo-graph-hidden", Some("text/plain".to_string()));

    //playback == sink, capture == src
    let graph_src = jack::Property::new("rnbo-graph-user-src", Some("text/plain".to_string()));
    let graph_sink = jack::Property::new("rnbo-graph-user-sink", Some("text/plain".to_string()));

    port_set_group(&c, &display_port, &hidden);
    port_set_group(&c, &midi_control_out, &hidden);
    port_set_group(&c, &midi_in, &hidden);
    port_set_group(&c, &system_display_port, &hidden);

    port_set_group(&c, &system_midi_capture_port, &hidden);
    port_set_group(&c, &midi_thru, &graph_src);

    let pretty = jack::Property::new("Move In MIDI", Some("text/plain".to_string()));
    port_set_pretty(&c, &midi_thru, &pretty);

    {
        let in1 = c
            .port_by_name("system:capture_1")
            .expect("error getting system:capture_1");
        let in2 = c
            .port_by_name("system:capture_2")
            .expect("error getting system:capture_2");

        let out1 = c
            .port_by_name("system:playback_1")
            .expect("error getting system:playback_1");
        let out2 = c
            .port_by_name("system:playback_2")
            .expect("error getting system:playback_2");

        let midi_out = c
            .port_by_name("system:midi_playback")
            .expect("error getting system:midi_playback");

        let midi_ext_in = c
            .port_by_name("system:midi_capture_ext")
            .expect("error getting system:midi_capture_ext");
        let midi_ext_out = c
            .port_by_name("system:midi_playback_ext")
            .expect("error getting system:midi_playback_ext");

        port_set_group(&c, &in1, &graph_src);
        port_set_group(&c, &in2, &graph_src);
        port_set_group(&c, &out1, &hidden);
        port_set_group(&c, &out2, &hidden);

        port_set_group(&c, &midi_out, &graph_sink);
        port_set_group(&c, &midi_ext_in, &graph_src);
        port_set_group(&c, &midi_ext_out, &graph_sink);

        let pretty = jack::Property::new("Move In Left", Some("text/plain".to_string()));
        port_set_pretty(&c, &in1, &pretty);
        let pretty = jack::Property::new("Move In Right", Some("text/plain".to_string()));
        port_set_pretty(&c, &in2, &pretty);

        let pretty = jack::Property::new("Move Out MIDI", Some("text/plain".to_string()));
        port_set_pretty(&c, &midi_out, &pretty);

        let pretty = jack::Property::new("External In MIDI", Some("text/plain".to_string()));
        port_set_pretty(&c, &midi_ext_in, &pretty);
        let pretty = jack::Property::new("External Out MIDI", Some("text/plain".to_string()));
        port_set_pretty(&c, &midi_ext_out, &pretty);
    }

    let mut display = MoveDisplay::new();
    let control = ConnectionControl {
        display_port: display_port.clone_unowned(),
        system_display_port,
        midi_in_port: midi_in.clone_unowned(),
        system_midi_out_port: system_midi_capture_port,
        disconnect_queue: disconnect_tx,
    };

    //volume control
    let volume = Arc::new(AtomicU8::new(0));
    let volumeclient = {
        let volume = volume.clone();
        let (volumeclient, _status) =
            jack::Client::new("move-volume", jack::ClientOptions::empty()).unwrap();
        let in1 = volumeclient
            .register_port("in1", AudioIn)
            .expect("error creating in1");
        let in2 = volumeclient
            .register_port("in2", AudioIn)
            .expect("error creating in2");

        let mut out1 = volumeclient
            .register_port("out1", AudioOut)
            .expect("error creating out1");
        let mut out2 = volumeclient
            .register_port("out2", AudioOut)
            .expect("error creating out2");

        port_set_group(&c, &in1, &graph_sink);
        port_set_group(&c, &in2, &graph_sink);

        let pretty = jack::Property::new("Move Out Left", Some("text/plain".to_string()));
        port_set_pretty(&c, &in1, &pretty);
        let pretty = jack::Property::new("Move Out Right", Some("text/plain".to_string()));
        port_set_pretty(&c, &in2, &pretty);

        port_set_group(&c, &out1, &hidden);
        port_set_group(&c, &out2, &hidden);

        let mut volume_last: u8 = 255;
        let mut volume_last_f: f32 = 1.0;

        let process_callback = move |_: &jack::Client, ps: &jack::ProcessScope| -> jack::Control {
            let out1 = out1.as_mut_slice(ps);
            let out2 = out2.as_mut_slice(ps);
            let in1 = in1.as_slice(ps);
            let in2 = in2.as_slice(ps);

            let volume = volume.load(Ordering::SeqCst);
            if volume == volume_last {
                if volume == 0xFF {
                    out1.clone_from_slice(in1);
                    out2.clone_from_slice(in2);
                } else {
                    for (o0, o1, i0, i1) in
                        itertools::izip!(out1.iter_mut(), out2.iter_mut(), in1.iter(), in2.iter())
                    {
                        *o0 = *i0 * volume_last_f;
                        *o1 = *i1 * volume_last_f;
                    }
                }
            } else {
                //fade
                let frames = ps.n_frames();

                let mut cur = volume_last as f32;
                let step = (volume as f32 - cur) / (frames as f32);

                for (o0, o1, i0, i1) in
                    itertools::izip!(out1.iter_mut(), out2.iter_mut(), in1.iter(), in2.iter())
                {
                    volume_last_f = (cur / 255.0).powf(4.0).clamp(0.0, 1.0);
                    *o0 = *i0 * volume_last_f;
                    *o1 = *i1 * volume_last_f;

                    cur += step;
                }

                volume_last = volume;
            }
            jack::Control::Continue
        };
        let process = jack::ClosureProcessHandler::new(process_callback);
        let volumeclient = volumeclient.activate_async((), process).unwrap();

        volumeclient
            .as_client()
            .connect_ports_by_name("move-volume:out1", "system:playback_1")
            .unwrap();
        volumeclient
            .as_client()
            .connect_ports_by_name("move-volume:out2", "system:playback_2")
            .unwrap();

        volumeclient
    };

    let size = display.size();
    let has_all_capabilities: bool = caps.all();
    if !has_all_capabilities {
        let _ = logger.warning("could not get requested capabilites");
    }

    let driver = Driver {
        display: display_port,
        midi_thru,
        midi_control_out,
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
            format!("{}:midi_control_out", name).as_str(),
            "system:midi_playback",
        )
        .unwrap();
        c.connect_ports_by_name("system:midi_capture", format!("{}:midi_in", name).as_str())
            .unwrap();
    }

    let version_path =
        PathBuf::from("/data/UserData/rnbo/share/rnbomovetakeover/package-version.txt");

    let package_version = if std::path::Path::exists(&version_path) {
        use std::io::Read;
        if let Ok(file) = std::fs::File::open(&version_path) {
            let mut reader = std::io::BufReader::new(file);
            let mut str = String::new();
            reader
                .read_to_string(&mut str)
                .expect("to read string from package-version.txt file");
            Some(str.trim().to_string())
        } else {
            None
        }
    } else {
        None
    };

    let state: std::sync::Arc<tokio::sync::Mutex<StateController>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(StateController::new(
            midi_out_tx,
            volume,
            package_version,
            config.clone(),
            has_all_capabilities,
        )));

    let display_future = async {
        use mousefood::prelude::*;

        let config = EmbeddedBackendConfig {
            flush_callback: Box::new(move |d: &mut MoveDisplay| {
                draw_tx
                    .send(DrawCommand {
                        data: d.framebuffer().clone(),
                    })
                    .unwrap();
            }),
            font_regular: font::CGA_LIGHT_8X16,
            font_bold: Some(font::CGA_8X16),
            ..Default::default()
        };

        let backend = EmbeddedBackend::new(&mut display, config);
        let mut terminal = ratatui::Terminal::new(backend).expect("to create terminal");
        let state = state.clone();

        loop {
            //frame rate
            tokio::time::sleep(Duration::from_millis(23)).await;

            let mut g = state.lock().await;
            let _ = terminal.clear();
            terminal
                .draw(|frame| {
                    g.render(frame);
                })
                .expect("to render frame");
            g.process_cmds().await;
        }
    };

    let process_midi = async {
        while let Some(midi) = midi_in_rx.recv().await {
            let mut c = state.lock().await;
            if c.handle_midi(midi.bytes()).await {
                break;
            }
        }
        //exit, sleep for a little so we can update the display
        tokio::time::sleep(Duration::from_millis(500)).await;
    };

    let inst_query: tokio::sync::Mutex<Option<Instant>> = tokio::sync::Mutex::new(None);
    let sets_query: tokio::sync::Mutex<Option<Instant>> = tokio::sync::Mutex::new(None);
    let views_query: tokio::sync::Mutex<Option<Instant>> = tokio::sync::Mutex::new(None);
    let set_current_query: tokio::sync::Mutex<Option<Instant>> = tokio::sync::Mutex::new(None);

    let inst_path_regex = Regex::new(r"^/rnbo/inst/\d+$").expect("to create instance regex");
    let set_view_regex =
        Regex::new(r"^/rnbo/inst/control/sets/views/list/\d+$").expect("to create set view regex");

    //http://c74rpi.local:5678

    async fn get_instances() -> Result<HashMap<usize, PatcherInst>, ()> {
        if let Ok(res) = reqwest::Client::new()
            .get("http://127.0.0.1:5678/rnbo/inst")
            .send()
            .await
        {
            let res = res.json().await.map_err(|_| ())?;
            //println!("got instances {:?}", res);
            if let Some(inst) = PatcherInst::parse_all(&res) {
                Ok(inst)
            } else {
                eprintln!("err parsing instances");
                Err(())
            }
        } else {
            eprintln!("err getting instances");
            Err(())
        }
    }

    async fn get_current_set_name() -> Result<Option<String>, ()> {
        if let Ok(res) = reqwest::Client::new()
            .get("http://127.0.0.1:5678/rnbo/inst/control/sets/current/name")
            .send()
            .await
        {
            let json: serde_json::Value = res.json().await.map_err(|_| ())?;
            Ok(match json.get("VALUE") {
                Some(serde_json::Value::String(s)) => Some(s.clone()),
                _ => None,
            })
        } else {
            eprintln!("err getting current set name");
            Err(())
        }
    }

    async fn get_string_range(path: &str) -> Option<Vec<String>> {
        if let Ok(res) = reqwest::Client::new().get(path).send().await {
            let res: Result<StringRange, _> = res.json().await;
            if let Ok(res) = res {
                return Some(res.range());
            }
        }
        None
    }

    async fn get_views() -> Result<Vec<ParamView>, ()> {
        let mut views = Vec::new();
        if let Ok(res) = reqwest::Client::new()
            .get("http://127.0.0.1:5678/rnbo/inst/control/sets/views/list")
            .send()
            .await
        {
            let json: serde_json::Value = res.json().await.map_err(|_| ())?;
            views = ParamView::parse_all(&json);
        }
        Ok(views)
    }

    let inst_query_future = async {
        loop {
            tokio::time::sleep(HTTP_QUERY_DELAY).await;
            {
                let mut g = sets_query.lock().await;
                if let Some(v) = g.deref() {
                    if *v <= Instant::now() {
                        if let Some(names) = get_string_range(
                            "http://127.0.0.1:5678/rnbo/inst/control/sets/load?RANGE",
                        )
                        .await
                        {
                            *g = None;
                            let mut g = state.lock().await;
                            g.set_set_names(names).await;
                        }

                        if let Some(names) = get_string_range(
                            "http://127.0.0.1:5678/rnbo/inst/control/sets/presets/load?RANGE",
                        )
                        .await
                        {
                            *g = None;
                            let mut g = state.lock().await;
                            g.set_set_preset_names(names).await;
                        }
                    }
                }
            }

            {
                let mut g = inst_query.lock().await;
                if let Some(v) = g.deref() {
                    if *v <= Instant::now() {
                        //println!("got instance update");
                        if let Ok(inst) = get_instances().await {
                            *g = None;
                            let mut g = state.lock().await;
                            g.set_instances(inst).await;

                            //look up views after instances have been looked up
                            {
                                let mut g = views_query.lock().await;
                                *g = Some(Instant::now() + HTTP_INITIAL_QUERY_DELAY);
                            }
                        }
                    }
                }
            }
            {
                let mut g = set_current_query.lock().await;
                if let Some(v) = g.deref() {
                    if *v <= Instant::now() {
                        if let Ok(name) = get_current_set_name().await {
                            *g = None;
                            let mut g = state.lock().await;
                            g.set_set_current_name(name).await;
                        }
                    }
                }
            }
            {
                let mut g = views_query.lock().await;
                if let Some(v) = g.deref() {
                    if *v <= Instant::now() {
                        if let Ok(views) = get_views().await {
                            *g = None;
                            let mut g = state.lock().await;
                            g.set_param_views(views).await;
                        }
                    }
                }
            }
        }
    };

    #[derive(Serialize, Deserialize, Debug, Default)]
    struct StringRangeItem {
        #[serde(rename = "VALS")]
        vals: Vec<String>,
    }

    #[derive(Serialize, Deserialize, Debug, Default)]
    struct StringRange {
        #[serde(rename = "RANGE")]
        range: Option<[StringRangeItem; 1]>,
    }

    impl StringRange {
        fn range(&self) -> Vec<String> {
            if let Some(range) = &self.range {
                range[0].vals.clone()
            } else {
                Vec::new()
            }
        }
    }

    let web_future = async {
        let state = state.clone();
        loop {
            if let Ok(res) = reqwest::Client::new()
                .get("http://127.0.0.1:5678/rnbo/info/version?VALUE")
                .send()
                .await
            {
                let version: serde_json::Value = res.json().await.unwrap();
                let version = version.get("VALUE").unwrap();
                if let serde_json::Value::String(version) = version {
                    let mut g = state.lock().await;
                    g.set_runner_version(version.as_str());
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
                            let mut g = state.lock().await;
                            g.set_ws(tx).await;
                        }

                        let timeout = Instant::now() + HTTP_INITIAL_QUERY_DELAY;

                        //do inst query
                        {
                            let mut g = inst_query.lock().await;
                            *g = Some(timeout);
                        }

                        //do sets query
                        {
                            let mut g = sets_query.lock().await;
                            *g = Some(timeout);
                        }

                        {
                            let mut g = views_query.lock().await;
                            *g = Some(timeout + HTTP_INITIAL_QUERY_DELAY);
                        }

                        //do set current query
                        {
                            let mut g = set_current_query.lock().await;
                            *g = Some(timeout);
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
                                                            Some(controller::SET_LOAD_ADDR) => {
                                                                let range: Result<StringRange, _> =
                                                                    serde_json::from_value(
                                                                        data.clone(),
                                                                    );
                                                                if let Ok(range) = range {
                                                                    let mut g = state.lock().await;
                                                                    let range = range.range();
                                                                    g.set_set_names(range).await;
                                                                }
                                                            }
                                                            Some(
                                                                controller::SET_PRESETS_LOAD_ADDR,
                                                            ) => {
                                                                let range: Result<StringRange, _> =
                                                                    serde_json::from_value(
                                                                        data.clone(),
                                                                    );
                                                                if let Ok(range) = range {
                                                                    let mut g = state.lock().await;
                                                                    let range = range.range();
                                                                    g.set_set_preset_names(range)
                                                                        .await;
                                                                }
                                                            }
                                                            _ => {
                                                                //println!("data {:?}", cmd);
                                                            }
                                                        }
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
                                                    //println!("path {:?}", path);
                                                    //added or removed
                                                    if inst_path_regex.is_match(path) {
                                                        let mut g = inst_query.lock().await;
                                                        *g = Some(Instant::now());
                                                    } else if set_view_regex.is_match(path) {
                                                        let mut g = views_query.lock().await;
                                                        *g = Some(
                                                            Instant::now()
                                                                + Duration::from_millis(100),
                                                        );
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

    let mut handles = tokio::task::JoinSet::new();

    for s in startup.iter() {
        let _ = logger.info(format!("spawning {}", s.cmd));

        //let state = state.clone();
        let mut cmd = tokio::process::Command::new(s.cmd.clone());
        let child = if let Some(args) = s.args.clone() {
            cmd.args(args)
        } else {
            &mut cmd
        }
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();

        if let Err(e) = child {
            let _ = logger.err(format!("failed to spawn child process {}", s.cmd));
            return Err(Box::new(e));
        }

        let mut child = child.unwrap();

        let stdout = child.stdout.take().expect("to get child stdout");
        let stderr = child.stderr.take().expect("to get child stderr");
        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let path = std::path::Path::new(s.cmd.as_str());
        let name = path
            .file_name()
            .expect("to get file name")
            .to_os_string()
            .into_string()
            .expect("to get string");
        handles.spawn(async move {
            let formatter = syslog::Formatter3164 {
                facility: syslog::Facility::LOG_USER,
                hostname: None,
                process: name.clone(),
                pid: child.id().unwrap_or(0),
            };
            let mut logger = syslog::unix(formatter).expect("to get syslog");
            let exit_status;
            loop {
                tokio::select! {
                    result = stdout_reader.next_line() => {
                        match result {
                            Ok(Some(line)) => {
                                let _ = logger.info(line);
                            },
                            Err(e) => {
                                //XXX should actually go in top level log but, whatever
                                let _ = logger.err(e);
                            },
                            _ => (),
                        }
                    }
                    result = stderr_reader.next_line() => {
                        match result {
                            Ok(Some(line)) => {
                                let _ = logger.err(line);
                            },
                            Err(e) => {
                                //XXX should actually go in top level log but, whatever
                                let _ = logger.err(e);
                            },
                            _ => (),
                        }
                    }
                    result = child.wait() => {
                        exit_status = result;
                        break // child process exited
                    }
                };
            }
            (name, exit_status)
        });
    }

    let children_future = {
        let state = state.clone();
        async move {
            if let Some(res) = handles.join_next().await {
                match res {
                    Ok((name, status)) => {
                        let _ = logger.err(format!(
                            "child process {} exited early with status {:?}",
                            name, status
                        ));
                        let mut g = state.lock().await;
                        g.display_child_process_error(name.as_str(), status).await;
                    }
                    Err(e) => {
                        let _ = logger.err(e);
                    }
                }
            }
            loop {
                //keep looping some user can see display and exit or start move
                tokio::time::sleep(Duration::from_millis(1000)).await;
            }
        }
    };

    let signal_future = async {
        tokio::signal::ctrl_c().await.unwrap();
    };
    tokio::select! {
        _ = display_future => (), _ = web_future => (),  _ = inst_query_future => (),
        _ = signal_future => (), _ = process_midi => (), _ = disconnect_future => (),
        _ = children_future => ()
    };

    let _ = volumeclient.deactivate();
    let _ = c.deactivate();

    Ok(())
}

async fn start_jack(
    j: &StartupProcess,
    exit_rx: tokio::sync::oneshot::Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    let j = j.clone();
    tokio::spawn(async move {
        let mut cmd = tokio::process::Command::new(j.cmd);

        let mut child = if let Some(args) = j.args {
            cmd.args(args)
        } else {
            &mut cmd
        }
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("to start jack");
        let stdout = child.stdout.take().expect("to get child stdout");
        let stderr = child.stderr.take().expect("to get child stderr");
        let mut stdout_reader = BufReader::new(stdout).lines();
        let mut stderr_reader = BufReader::new(stderr).lines();

        let formatter = syslog::Formatter3164 {
            facility: syslog::Facility::LOG_USER,
            hostname: None,
            process: "jackd".to_string(),
            pid: child.id().unwrap_or(0),
        };
        let mut logger = syslog::unix(formatter).expect("to get syslog");
        tokio::select! {
            _ = exit_rx => {
                let _ = child.kill().await;
            },
            _ = async {
                loop {
                    tokio::select! {
                        result = stdout_reader.next_line() => {
                            match result {
                                Ok(Some(line)) => {
                                    let _ = logger.info(line);
                                },
                                Err(e) => {
                                    //XXX should actually go in top level log but, whatever
                                    let _ = logger.err(e);
                                },
                                _ => (),
                            }
                        }
                        result = stderr_reader.next_line() => {
                            match result {
                                Ok(Some(line)) => {
                                    let _ = logger.err(line);
                                },
                                Err(e) => {
                                    //XXX should actually go in top level log but, whatever
                                    let _ = logger.err(e);
                                },
                                _ => (),
                            }
                        }
                        _ = child.wait() => {
                            let _ = logger.info("jack exited");
                            break // child process exited
                        }
                    };
                }
            } => {}
        };
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    //set HOME if it doesn't already exist
    if std::env::var_os("HOME").is_none() {
        std::env::set_var("HOME", "/data/UserData/");
    }

    //hack, for some reason running setcap on this executable will unset TMPDIR after it starts
    //set it again
    std::env::set_var("TMPDIR", "/data/UserData/Scratch/");

    let args = Args::parse();
    let homedir = home::home_dir().expect("to get home directory");

    let config = args.config;
    let config = if let Some(config) = config.strip_prefix("~/") {
        let mut p = homedir.clone();
        p.push(config);
        p
    } else {
        PathBuf::from(config)
    };

    //request changes to resource limits
    let memlock = setrlimit(
        rlimit::Resource::MEMLOCK,
        rlimit::INFINITY,
        rlimit::INFINITY,
    )
    .is_ok();
    let rtprio = setrlimit(rlimit::Resource::RTPRIO, 95, 95).is_ok();

    let caps = Caps { memlock, rtprio };

    let mut tostartup: Vec<StartupProcess> = Vec::new();
    let mut jackstartup: Option<StartupProcess> = None;

    if let Some(startup) = args.startup {
        let startup = PathBuf::from(startup)
            .canonicalize()
            .expect("to parse startup file path");

        let startup = std::fs::read_to_string(startup).expect("Unable to read startup file");
        let startup: StartupConfig = serde_json::from_str(&startup)?;
        jackstartup = startup.jack.clone();

        if let Some(s) = startup.apps {
            tostartup = s.clone();
        }
    }

    let name = "move-control";

    let formatter = syslog::Formatter3164 {
        facility: syslog::Facility::LOG_USER,
        hostname: None,
        process: "rnbomovecontrol".to_string(),
        pid: std::process::id(),
    };
    let mut logger = syslog::unix(formatter).expect("to get syslog");
    let _ = logger.info("starting");

    let (exit_tx, exit_rx) = tokio::sync::oneshot::channel();
    let mut jack_handle = if let Some(j) = jackstartup.clone() {
        let _ = logger.info("starting jack");
        Some(start_jack(&j, exit_rx).await)
    } else {
        None
    };

    //wait until jack exists
    let pollms = 500;
    tokio::time::sleep(Duration::from_millis(pollms)).await;
    loop {
        tokio::time::sleep(Duration::from_millis(10)).await;
        if let Some(jack) = &jack_handle {
            if jack.is_finished() {
                //TODO retry jack?
                let _ = logger.err("failed to start jack");
                jack_handle = None;
                break;
            }
        }
        if let Ok((c, _status)) = Client::new(name, ClientOptions::NO_START_SERVER) {
            if let Err(e) = with_client(c, &mut logger, &tostartup, &config, caps).await {
                let _ = logger.err(e);
            }
            break;
        } else {
            tokio::time::sleep(Duration::from_millis(pollms - 10)).await;
        }
    }

    //kill jack if we spawned it
    if let Some(jack) = jack_handle {
        let _ = logger.info("killing jack");
        let _ = exit_tx.send(());
        tokio::time::sleep(Duration::from_millis(100)).await;
        for _i in 0..5 {
            if jack.is_finished() {
                return Ok(());
            }
            let _ = logger.err("jack isn't finished");
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
        let _ = logger.err("failed to cleanly stop jack");
        //XXX
    }

    Ok(())
}
