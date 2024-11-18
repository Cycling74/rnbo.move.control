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

const TRANSPORT_ROLLING_ADDR: &str = "/rnbo/jack/transport/rolling";
const TRANSPORT_BPM_ADDR: &str = "/rnbo/jack/transport/bpm";

pub const SET_LOAD_ADDR: &str = "/rnbo/inst/control/sets/load";
pub const SET_PRESETS_LOAD_ADDR: &str = "/rnbo/inst/control/sets/presets/load";

const VOLUME_WHEEL_ENCODER: usize = 9;
const JOG_WHEEL_ENCODER: usize = 10;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
enum MoveColor {
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
struct ParamUpdate {
    instance: usize,
    index: usize,
}

#[derive(Clone, Debug, PartialEq)]
enum Events {
    BtnDown(Button),
    EncLeft(usize),
    EncRight(usize),

    ParamUpdate(ParamUpdate),
    Transport(bool),
    Tempo(f32),

    SetNamesChanged,
    SetPresetNamesChanged,
}

const MENU_ITEMS: [&'static str; 3] = ["Set Presets", "Sets", "Patcher Instances"];
const SET_PRESETS_INDEX: usize = 0;
const SETS_INDEX: usize = 1;
const PATCHER_INSTANCES_INDEX: usize = 2;

smlang::statemachine! {
    states_attr: #[derive(Clone)],
    transitions: {
        *Init + BtnDown(Button::Menu) = Menu(0),
        PromptPower + BtnDown(Button::JogWheel) = PowerOff,
        PromptPower + BtnDown(Button::Back) = Menu(0),

        //nav
        Menu(usize) + EncRight(JOG_WHEEL_ENCODER) [*state + 1 < MENU_ITEMS.len()] = Menu(*state + 1),
        Menu(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = Menu(*state - 1),

        //select
        Menu(usize) + BtnDown(Button::JogWheel) [*state == SETS_INDEX && ctx.sets_len() > 0] = SetsList(0),
        Menu(usize) + BtnDown(Button::JogWheel) [*state == SET_PRESETS_INDEX && ctx.set_presets_len() > 0] = SetPresetsList(0),
        Menu(usize) + BtnDown(Button::JogWheel) [*state == PATCHER_INSTANCES_INDEX && ctx.patcher_instances_len() > 0] = PatcherInstances(0),

        SetsList(usize) + BtnDown(Button::Back) = Menu(SETS_INDEX),
        SetsList(usize) + EncRight(JOG_WHEEL_ENCODER) [ctx.sets_len() > *state + 1] = SetsList(*state + 1),
        SetsList(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = SetsList(*state - 1),
        SetsList(usize) + BtnDown(Button::JogWheel) / ctx.set_select(*state).await; = Menu(SET_PRESETS_INDEX),
        //SetsList(usize) + EncRight(JOG_WHEEL_ENCODER) [ctx.sets_len() == 0] = Menu(MenuItems::Sets), //abort

        SetPresetsList(usize) + BtnDown(Button::Back) = Menu(SET_PRESETS_INDEX),
        SetPresetsList(usize) + EncRight(JOG_WHEEL_ENCODER) [ctx.set_presets_len() > *state + 1] = SetPresetsList(*state + 1),
        SetPresetsList(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = SetPresetsList(*state - 1),
        SetPresetsList(usize) + BtnDown(Button::JogWheel) / ctx.set_preset_select(*state).await;,

        PatcherInstances(usize) + BtnDown(Button::Back) = Menu(PATCHER_INSTANCES_INDEX),

        _ + BtnDown(Button::PowerShort) / ctx.send_power_cmd(PowerCommand::ClearShortPress); = PromptPower,
        _ + BtnDown(Button::PowerLong) / ctx.send_power_cmd(PowerCommand::ClearLongPress); = PowerOff,
        _ + Tempo(_) / ctx.update_tempo(*event);,
        _ + Transport(_) / ctx.update_transport(*event);,
        _ + BtnDown(Button::Play) / ctx.toggle_transport().await;,
    }
}

pub struct StateController {
    pub instances: HashMap<usize, PatcherInst>,
    pub params: HashMap<String, usize>,
    pub selected_param: Option<(usize, usize)>,
    sysex: Vec<u8>,
    statemachine: StateMachine,
}

struct Context {
    display: Rc<Mutex<MoveDisplay>>,
    midi_out_queue: sync_mpsc::SyncSender<Midi>,
    bpm: f32,
    rolling: bool,
    ws_tx: Option<SplitSink<WebSocket, Message>>,
    set_names: Vec<String>,
    set_preset_names: Vec<String>,
    patcher_instance_names: Vec<String>,
    patcher_instance_indexes: Vec<usize>,
    set_selected: Option<String>,
}

impl Context {
    fn new(
        midi_out_queue: sync_mpsc::SyncSender<Midi>,
        display: &mut Rc<Mutex<MoveDisplay>>,
    ) -> Self {
        //send a reset
        let _ = midi_out_queue.send(Midi::reset());

        Self {
            display: display.clone(),
            midi_out_queue,
            bpm: 0f32,
            rolling: false,
            ws_tx: None,
            set_names: Vec::new(),
            set_preset_names: Vec::new(),
            set_selected: None,
            patcher_instance_names: Vec::new(),
            patcher_instance_indexes: Vec::new(),
        }
    }

    fn send_power_cmd(&mut self, cmd: PowerCommand) {
        for m in power_sysex(cmd).into_iter() {
            let _ = self.midi_out_queue.send(m);
        }
    }

    fn sets_len(&self) -> usize {
        self.set_names.len()
    }

    fn set_presets_len(&self) -> usize {
        self.set_preset_names.len()
    }

    fn patcher_instances_len(&self) -> usize {
        self.patcher_instance_names.len()
    }

    async fn set_select(&mut self, index: usize) {
        self.set_selected = self.set_names.get(index).map(|s| s.clone());
        if let Some(name) = &self.set_selected {
            let msg = OscMessage {
                addr: SET_LOAD_ADDR.to_string(),
                args: vec![OscType::String(name.clone())],
            };
            self.send_osc(msg).await;
        }
    }

    async fn set_preset_select(&mut self, index: usize) {
        let selected = self.set_preset_names.get(index).map(|s| s.clone());
        if let Some(name) = &selected {
            let msg = OscMessage {
                addr: SET_PRESETS_LOAD_ADDR.to_string(),
                args: vec![OscType::String(name.clone())],
            };
            self.send_osc(msg).await;
        }
    }

    fn set_set_names(&mut self, names: &Vec<String>) {
        //we always want to keep track of the names but we might not change state
        let mut names = names.clone();
        names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        self.set_names = names;

        //TODO change selected index?
    }

    fn set_set_preset_names(&mut self, names: &Vec<String>) {
        let mut names = names.clone();
        names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
        self.set_preset_names = names;
    }

    fn set_names(&self) -> &Vec<String> {
        &self.set_names
    }

    fn set_preset_names(&self) -> &Vec<String> {
        &self.set_preset_names
    }

    fn set_patchers(&mut self, instances: &HashMap<usize, PatcherInst>) {
        self.patcher_instance_indexes = instances.keys().map(|k| *k).collect();
        self.patcher_instance_indexes.sort();
        self.patcher_instance_names.clear();
        for i in self.patcher_instance_indexes.iter() {
            self.patcher_instance_names.push(format!(
                "{}: {}",
                i,
                instances.get(&i).unwrap().name()
            ));
        }
    }

    fn patcher_instance_names(&self) -> &Vec<String> {
        &self.patcher_instance_names
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
                MoveColor::Green
            } else {
                MoveColor::LightGray
            } as u8,
        );
    }

    async fn toggle_transport(&mut self) {
        let msg = OscMessage {
            addr: TRANSPORT_ROLLING_ADDR.to_string(),
            args: vec![OscType::Bool(!self.rolling)],
        };
        self.send_osc(msg).await;
    }

    async fn send_osc(&mut self, msg: OscMessage) {
        let packet = OscPacket::Message(msg);
        if let Ok(msg) = rosc::encoder::encode(&packet) {
            if let Some(ws) = self.ws_tx.as_mut() {
                let _ = ws.send(Message::Binary(msg)).await;
            }
        }
    }
}

impl StateController {
    pub fn new(
        midi_out_queue: sync_mpsc::SyncSender<Midi>,
        display: &mut Rc<Mutex<MoveDisplay>>,
    ) -> Self {
        let mut context = Context::new(midi_out_queue, display);

        context.light_button(MENU_MIDI, MoveColor::LightGray as _);
        context.light_button(PLAY_MIDI, MoveColor::LightGray as _);

        Self {
            instances: HashMap::new(),
            params: HashMap::new(),
            selected_param: None,
            sysex: Vec::new(),
            statemachine: StateMachine::new(context),
        }
    }

    pub async fn set_ws(&mut self, mut ws: SplitSink<WebSocket, Message>) {
        //query values
        for addr in [TRANSPORT_ROLLING_ADDR, TRANSPORT_BPM_ADDR] {
            let msg = OscMessage {
                addr: addr.to_string(),
                args: Vec::new(),
            };
            let packet = OscPacket::Message(msg);
            if let Ok(msg) = rosc::encoder::encode(&packet) {
                let _ = ws.send(Message::Binary(msg)).await;
            }
        }
        self.context_mut().ws_tx = Some(ws);
    }

    pub fn set_state(&mut self, instances: HashMap<usize, PatcherInst>) {
        self.context_mut().set_patchers(&instances);

        let mut params: HashMap<String, usize> = HashMap::new();
        for (index, v) in instances.iter() {
            for p in v.params().iter() {
                params.insert(p.addr().to_string(), *index);
            }
        }
        self.instances = instances;
        self.params = params;
    }

    pub async fn set_set_names(&mut self, names: &Vec<String>) {
        self.context_mut().set_set_names(names);
        self.handle_event(Events::SetNamesChanged).await;
    }

    pub async fn set_set_preset_names(&mut self, names: &Vec<String>) {
        self.context_mut().set_set_preset_names(names);
        self.handle_event(Events::SetPresetNamesChanged).await;
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
                TRANSPORT_ROLLING_ADDR => {
                    if let OscType::Bool(rolling) = msg.args[0] {
                        self.handle_event(Events::Transport(rolling)).await;
                    }
                }
                TRANSPORT_BPM_ADDR => {
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
                        self.handle_event(Events::BtnDown(Button::PowerShort)).await;
                    }
                    if status & 0b1_0000 != 0 {
                        self.handle_event(Events::BtnDown(Button::PowerLong)).await;
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
                    /*
                     * TODO
                    self.handle_event(Events::Btn(Btn(
                        Button::EncoderTouch(bytes[1] as usize),
                        bytes[2] != 0,
                    )))
                    .await;
                    */
                }
            }
            0xB0 => {
                self.sysex.clear();
                match bytes[1] {
                    //jog wheel btn
                    0x03 if bytes[2] != 0 => {
                        self.handle_event(Events::BtnDown(Button::JogWheel)).await;
                    }
                    0x0e => match bytes[2] {
                        1 => {
                            self.handle_event(Events::EncRight(JOG_WHEEL_ENCODER)).await;
                        }
                        127 => {
                            self.handle_event(Events::EncLeft(JOG_WHEEL_ENCODER)).await;
                        }
                        _ => (),
                    },
                    //hamburger
                    MENU_MIDI if bytes[2] != 0 => {
                        self.handle_event(Events::BtnDown(Button::Menu)).await;
                    }
                    //menu back button
                    BACK_MIDI if bytes[2] != 0 => {
                        self.handle_event(Events::BtnDown(Button::Back)).await;
                    }
                    //play button
                    PLAY_MIDI if bytes[2] != 0 => {
                        self.handle_event(Events::BtnDown(Button::Play)).await;
                    }
                    0xf4 => {
                        //volume jog
                    }

                    //param encoders
                    index @ 71..=78 => {
                        let index = (index - 71) as usize;
                        match bytes[2] {
                            1 => {
                                self.handle_event(Events::EncLeft(index)).await;
                            }
                            127 => {
                                self.handle_event(Events::EncLeft(index)).await;
                            }
                            _ => (),
                        }

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
        if let Some(ns) = self.statemachine.process_event(e).await {
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
                States::Menu(selected) => {
                    let selected: usize = *selected;
                    {
                        let display = self.locked_display().await;
                        draw_menu(display, &"RNBO On Move", &MENU_ITEMS, selected);
                    }
                    self.context_mut().light_button(MENU_MIDI, 0);
                    self.context_mut().light_button(BACK_MIDI, 0);
                }
                States::SetsList(selected) => {
                    let selected = *selected;
                    {
                        let display = self.locked_display().await;
                        draw_menu(display, &"Load Set", self.context().set_names(), selected);
                    }

                    self.context_mut()
                        .light_button(MENU_MIDI, MoveColor::Black as _);
                    self.context_mut()
                        .light_button(BACK_MIDI, MoveColor::LightGray as _);
                }
                States::SetPresetsList(selected) => {
                    let selected = *selected;
                    {
                        let display = self.locked_display().await;
                        draw_menu(
                            display,
                            &"Load Set Preset",
                            self.context().set_preset_names(),
                            selected,
                        );
                    }

                    self.context_mut()
                        .light_button(MENU_MIDI, MoveColor::Black as _);
                    self.context_mut()
                        .light_button(BACK_MIDI, MoveColor::LightGray as _);
                }
                States::PatcherInstances(selected) => {
                    let selected = *selected;
                    {
                        let display = self.locked_display().await;
                        draw_menu(
                            display,
                            &"Patcher Instances",
                            self.context().patcher_instance_names(),
                            selected,
                        );
                    }

                    self.context_mut()
                        .light_button(MENU_MIDI, MoveColor::Black as _);
                    self.context_mut()
                        .light_button(BACK_MIDI, MoveColor::LightGray as _);
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

fn draw_menu<D: DerefMut<Target = MoveDisplay>, S: AsRef<str>>(
    mut display: D,
    title: &str,
    items: &[S],
    selected: usize,
) {
    use embedded_layout::{layout::linear::LinearLayout, prelude::*};
    let text_style = MonoTextStyle::new(&profont::PROFONT_12_POINT, BinaryColor::On);

    display.clear(BinaryColor::Off).unwrap();
    let display_area = display.bounding_box();

    let mut list: [String; 3] = Default::default();

    //try to keep 3 on screen, select indicator may need to move to first or last item depending
    let start = if selected == 0 || items.len() <= 3 {
        0
    } else if selected + 1 >= items.len() {
        items.len() - 3
    } else {
        selected - 1
    };

    for (index, (l, item)) in list
        .iter_mut()
        .zip(items.iter().skip(start).take(3))
        .enumerate()
    {
        *l = if index + start == selected {
            format!("> {}", item.as_ref())
        } else {
            format!("  {}", item.as_ref())
        }
        .to_string();

        //make strings all length 16
        if l.len() > 16 {
            //add ellipsis
            l.truncate(14);
            l.push_str("..");
        } else if l.len() < 16 {
            //add whitespace
            l.reserve(16 - l.len());
            while l.len() < 16 {
                l.push(' ');
            }
        }
    }

    LinearLayout::vertical(
        Chain::new(Text::new(title, Point::zero(), text_style)).append(
            LinearLayout::vertical(
                Chain::new(Text::new(list[0].as_str(), Point::zero(), text_style))
                    .append(Text::new(list[1].as_str(), Point::zero(), text_style))
                    .append(Text::new(list[2].as_str(), Point::zero(), text_style)),
            )
            .with_alignment(horizontal::Left)
            .align_to(&display_area, horizontal::Left, vertical::Center)
            .arrange(),
        ),
    )
    .with_alignment(horizontal::Center)
    .arrange()
    .align_to(&display_area, horizontal::Left, vertical::Top)
    .draw(display.deref_mut())
    .unwrap();
}
