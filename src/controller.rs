use {
    crate::{display::MoveDisplay, midi::Midi, patcher::PatcherInst},
    embedded_graphics::{
        mono_font::MonoTextStyle,
        pixelcolor::BinaryColor,
        prelude::*,
        text::{Alignment, Text},
    },
    futures_util::{stream::SplitSink, SinkExt, StreamExt, TryStreamExt},
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
    tokio::sync::{Mutex, MutexGuard},
};

const MENU_MIDI: u8 = 0x32;
const BACK_MIDI: u8 = 0x33;
const PLAY_MIDI: u8 = 0x55;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
enum RGBColor {
    Black = 0,

    FullWhite = 120, // Full brightness white (FFF, "white" below is CCC)

    White = 122,
    LightGray = 123,
    DarkGray = 124,

    Blue = 125,
    Green = 126,
    Red = 127,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
enum PowerCommand {
    ///Power off the device immediately; `shutdown` should be sent before. if shutdown has not been sent, powering off is delayed for 5 seconds.
    PowerOff = 1,
    /// Reset the power button state of a short press
    ClearShortPress = 2,
    /// Request a power state update via system MIDI event
    RequestStateUpdate = 3,
    /// Power off the device and auto power on after 1s
    Reboot = 4,
    /// Reset the power button state of a long press
    ClearLongPress = 5,
    /// Initiate XMOS shutdown and animation; `powerOff` required after this. If `powerOff` is not sent, the device is powered off after 30 seconds. `powerOff` will be called by MoveXmosPower as part of the operating systems shutdown sequence.
    Shutdown = 6,
}

fn power_sysex(cmd: PowerCommand) -> [Midi; 3] {
    [
        Midi::new(&[0xF0, 0x00, 0x21]),
        Midi::new(&[0x1D, 0x01, 0x01]),
        Midi::new(&[0x39, cmd as u8, 0xF7]),
    ]
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Button {
    JogWheel,
    Back,
    Shift,
    PowerLong,
    PowerShort,
    Menu,
    Play,
    EncoderTouch(usize),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Btn(Button, bool);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ParamUpdate {
    instance: usize,
    index: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Encoder(usize, u8);

#[derive(Copy, Clone, Debug, PartialEq)]
enum Events {
    Btn(Btn),
    ParamUpdate(ParamUpdate),
    Encoder(Encoder),
    Transport(bool),
    Tempo(f32),
}

smlang::statemachine! {
    states_attr: #[derive(Clone)],
    transitions: {
        *Init + Btn(Btn(Button::Menu, true)) = Menu,
        PromptPower + Btn(Btn(Button::JogWheel, true)) = PowerOff,
        PromptPower + Btn(Btn(Button::Back, true))  = Menu,
        _ + Btn(Btn(Button::PowerShort, _)) / ctx.send_power_cmd(PowerCommand::ClearShortPress); = PromptPower,
        _ + Btn(Btn(Button::PowerLong, _)) / ctx.send_power_cmd(PowerCommand::ClearLongPress); = PowerOff,
        _ + Tempo(_) / ctx.update_tempo(*event);,
        _ + Transport(_) / ctx.update_transport(*event);,
    }
}

pub struct StateController {
    pub instances: HashMap<usize, PatcherInst>,
    pub params: HashMap<String, usize>,
    pub selected_param: Option<(usize, usize)>,
    ws_tx: Option<SplitSink<WebSocket, Message>>,
    sysex: Vec<u8>,
    statemachine: StateMachine,
    set_names: Vec<String>,
}

struct Context {
    display: Rc<Mutex<MoveDisplay>>,
    midi_out_queue: sync_mpsc::SyncSender<Midi>,
    bpm: f32,
    rolling: bool,
}

impl Context {
    fn send_power_cmd(&mut self, cmd: PowerCommand) {
        for m in power_sysex(cmd).into_iter() {
            let _ = self.midi_out_queue.send(m);
        }
    }

    fn light_button(&mut self, btn: u8, val: u8) {
        let _ = self.midi_out_queue.send(Midi::cc(btn, val, 0));
    }

    fn update_tempo(&mut self, v: f32) {
        self.bpm = v;
        //TODO
    }

    fn update_transport(&mut self, on: bool) {
        self.rolling = on;
        self.light_button(
            PLAY_MIDI,
            if on {
                RGBColor::Green
            } else {
                RGBColor::LightGray
            } as u8,
        );
    }
}

impl StateController {
    pub fn new(
        midi_out_queue: sync_mpsc::SyncSender<Midi>,
        display: &mut Rc<Mutex<MoveDisplay>>,
    ) -> Self {
        let mut context = Context {
            display: display.clone(),
            midi_out_queue,
            bpm: 0f32,
            rolling: false,
        };

        context.light_button(MENU_MIDI, RGBColor::LightGray as _);
        context.light_button(PLAY_MIDI, RGBColor::LightGray as _);

        Self {
            instances: HashMap::new(),
            params: HashMap::new(),
            selected_param: None,
            ws_tx: None,
            sysex: Vec::new(),
            statemachine: StateMachine::new(context),
            set_names: Vec::new(),
        }
    }

    pub async fn set_ws(&mut self, mut ws: SplitSink<WebSocket, Message>) {
        //query values
        for addr in ["/rnbo/jack/transport/rolling", "/rnbo/jack/transport/bpm"] {
            let msg = OscMessage {
                addr: addr.to_string(),
                args: Vec::new(),
            };
            let packet = OscPacket::Message(msg);
            if let Ok(msg) = rosc::encoder::encode(&packet) {
                let _ = ws.send(Message::Binary(msg)).await;
            }
        }
        self.ws_tx = Some(ws);
    }

    pub fn set_state(&mut self, instances: HashMap<usize, PatcherInst>) {
        let mut params: HashMap<String, usize> = HashMap::new();
        for (index, v) in instances.iter() {
            for p in v.params().iter() {
                params.insert(p.addr().to_string(), *index);
            }
        }
        self.instances = instances;
        self.params = params;
    }

    pub fn set_set_names(&mut self, names: &Vec<String>) {
        self.set_names = names.clone();
    }

    pub async fn select_param(&mut self, v: Option<(usize, usize)>) {
        self.selected_param = v;
        self.render_param().await;
    }

    pub async fn render_param(&mut self) {
        let mut display = self.locked_display().await;
        let style = MonoTextStyle::new(&profont::PROFONT_12_POINT, BinaryColor::On);
        display.clear(BinaryColor::Off).unwrap();
        let size = display.size();

        if let Some((inst, param)) = self.selected_param {
            if let Some(inst) = self.instances.get(&inst) {
                if let Some(param) = inst.params().get(param) {
                    let s = format!("{}\n{}", param.name(), param.render_value());
                    Text::with_alignment(
                        s.as_str(),
                        Point::new(size.width as i32 / 2, size.height as i32 / 2),
                        style,
                        Alignment::Center,
                    )
                    .draw(display.deref_mut())
                    .unwrap();
                }
            }
        }
    }

    pub async fn handle_osc(&mut self, msg: &OscMessage) {
        if msg.args.len() == 1 {
            println!("got osc {}", msg.addr);
            let mut update = None;
            match msg.addr.as_str() {
                "/rnbo/jack/transport/rolling" => {
                    if let OscType::Bool(rolling) = msg.args[0] {
                        self.handle_event(Events::Transport(rolling)).await;
                    }
                }
                "/rnbo/jack/transport/bpm" => {
                    if let Some(bpm) = match &msg.args[0] {
                        OscType::Double(v) => Some(*v as f32),
                        OscType::Float(v) => Some(*v),
                        _ => None,
                    } {
                        self.handle_event(Events::Tempo(bpm)).await;
                    }
                }
                _ => {
                    if let Some(instance) = self.params.get(&msg.addr) {
                        if let Some(inst) = self.instances.get_mut(&instance) {
                            if let Some(index) = match &msg.args[0] {
                                OscType::Double(v) => inst.update_param_f64(&msg.addr, *v),
                                OscType::Float(v) => inst.update_param_f64(&msg.addr, *v as f64),
                                OscType::Int(v) => inst.update_param_f64(&msg.addr, *v as f64),
                                OscType::String(v) => inst.update_param_s(&msg.addr, v),
                                _ => None,
                            } {
                                update = Some((*instance, index));
                            }
                        }
                    }

                    if let Some((instance, index)) = update {
                        self.handle_event(Events::ParamUpdate(ParamUpdate { instance, index }))
                            .await;
                    }
                }
            }
        }
    }

    pub async fn handle_sysex(&mut self) {
        let sysex: Vec<u8> = std::mem::take(&mut self.sysex);
        match sysex[0..6] {
            [0x00, 0x21, 0x1d, 0x01, 0x01, 0x3a] => {
                //println!("power sysex {:02x?}", sysex);
                if let Some(status) = sysex.get(6) {
                    if status & 0b1000 != 0 {
                        self.handle_event(Events::Btn(Btn(Button::PowerShort, true)))
                            .await;
                    }
                    if status & 0b1_0000 != 0 {
                        self.handle_event(Events::Btn(Btn(Button::PowerLong, true)))
                            .await;
                    }
                }
            }
            _ => {
                println!("unhandled sysex {:02x?}", sysex);
            }
        }
    }

    pub async fn handle_midi(&mut self, bytes: &[u8; 3]) {
        println!("got midi {:02x?}", bytes);

        //volume 0x08
        //jog 0x09

        match bytes[0] {
            0x90 => {
                self.sysex.clear();
                if bytes[1] < 10 {
                    //0..7 params
                    //8 volume
                    //9 jog wheel
                    self.handle_event(Events::Btn(Btn(
                        Button::EncoderTouch(bytes[1] as usize),
                        bytes[2] != 0,
                    )))
                    .await;
                }
            }
            0xB0 => {
                self.sysex.clear();
                match bytes[1] {
                    //jog wheel btn
                    0x03 => {
                        self.handle_event(Events::Btn(Btn(Button::JogWheel, bytes[2] != 0)))
                            .await;
                    }
                    0x0e => {
                        //jog wheel action
                    }
                    //hamburger
                    MENU_MIDI => {
                        self.handle_event(Events::Btn(Btn(Button::Menu, bytes[2] != 0)))
                            .await;
                    }
                    //menu back button
                    BACK_MIDI => {
                        self.handle_event(Events::Btn(Btn(Button::Back, bytes[2] != 0)))
                            .await;
                    }
                    //play button
                    PLAY_MIDI => {
                        self.handle_event(Events::Btn(Btn(Button::Play, bytes[2] != 0)))
                            .await;
                    }
                    0xf4 => {
                        //volume jog
                    }

                    //param encoders
                    index @ 71..=78 => {
                        self.handle_event(Events::Encoder(Encoder(
                            (index - 71) as usize,
                            bytes[2],
                        )))
                        .await;

                        /*
                        let inst = 0;
                        let index = (index - 71) as usize;
                        let v = bytes[2];
                        //left == 127
                        //right == 1
                        if let Some(msg) = self.render_osc(inst, index, v as isize) {
                            let packet = OscPacket::Message(msg);
                            if let Ok(msg) = rosc::encoder::encode(&packet) {
                                let mut tx = ws_tx.lock().await;
                                if let Some(tx) = tx.deref_mut() {
                                    let _ = tx.send(Message::Binary(msg)).await;
                                }
                            }
                        }
                        if self.selected_param != Some((0, index)) {
                            self.select_param(Some((0, index))).await;
                        }
                        */
                    }
                    _ => (),
                }
            }
            0xF0 => {
                self.sysex.push(bytes[1]);
                self.sysex.push(bytes[2]);
            }
            0xF7 => {
                self.handle_sysex().await;
            }
            _ => {
                if bytes[0] & 0x80 != 0 {
                    self.sysex.clear();
                } else if self.sysex.len() > 0 {
                    //active sysex
                    if bytes[1] == 0xF7 {
                        self.sysex.push(bytes[0]);
                        self.handle_sysex().await;
                    } else if bytes[2] == 0xF7 {
                        self.sysex.push(bytes[0]);
                        self.sysex.push(bytes[1]);
                        self.handle_sysex().await;
                    } else {
                        self.sysex.extend_from_slice(bytes.as_slice());
                    }
                }
            }
        }
    }

    pub fn render_osc(&mut self, inst: usize, param: usize, v: isize) -> Option<OscMessage> {
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

    async fn display_centered(&mut self, text: &str) {
        let mut display = self.locked_display().await;
        let style = MonoTextStyle::new(&profont::PROFONT_12_POINT, BinaryColor::On);
        display.clear(BinaryColor::Off).unwrap();
        let size = display.size();

        Text::with_alignment(
            text,
            Point::new(size.width as i32 / 2, size.height as i32 / 2),
            style,
            Alignment::Center,
        )
        .draw(display.deref_mut())
        .unwrap();
    }

    async fn handle_event(&mut self, e: Events) {
        if let Some(ns) = self.statemachine.process_event(e) {
            //got new state
            match ns {
                States::PowerOff => {
                    self.display_centered("Powering Down").await;
                    self.context_mut().light_button(MENU_MIDI, 0);
                    self.context_mut().light_button(BACK_MIDI, 0);
                    //leave some time for it do draw
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    self.context_mut().send_power_cmd(PowerCommand::PowerOff);
                }
                States::PromptPower => {
                    self.context_mut().light_button(MENU_MIDI, 0);
                    self.context_mut().light_button(BACK_MIDI, 127);
                    self.display_centered("Press wheel to\nshut down").await;
                }
                States::Menu => {
                    self.context_mut().light_button(MENU_MIDI, 0);
                    self.context_mut().light_button(BACK_MIDI, 0);
                    self.display_centered("Menu TODO").await;
                }
                _ => (),
            }
        }
    }

    async fn locked_display(&self) -> MutexGuard<MoveDisplay> {
        self.context().display.lock().await
    }

    fn send_midi(&mut self, midi: Midi) {
        let _ = self.context_mut().midi_out_queue.send(midi);
    }

    fn context(&self) -> &Context {
        self.statemachine.context()
    }

    fn context_mut(&mut self) -> &mut Context {
        self.statemachine.context_mut()
    }
}
