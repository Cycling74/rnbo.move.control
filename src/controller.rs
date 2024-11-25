use {
    crate::{
        config::Config,
        display::{MoveDisplay, DISPLAY_HEIGHT, DISPLAY_WIDTH},
        midi::Midi,
        param::Param,
        patcher::PatcherInst,
    },
    embedded_graphics::{
        mono_font::MonoTextStyle,
        pixelcolor::BinaryColor,
        prelude::*,
        primitives::{PrimitiveStyleBuilder, Rectangle},
        text::{Alignment, Text},
    },
    futures_util::{stream::SplitSink, SinkExt, StreamExt, TryStreamExt},
    palette::{Darken, Srgb},
    reqwest_websocket::{Message, RequestBuilderExt, WebSocket},
    rosc::{OscMessage, OscPacket, OscType},
    std::{
        cmp::{Ordering, PartialEq, PartialOrd},
        collections::HashMap,
        error::Error,
        fs::File,
        io::BufReader,
        ops::{Deref, DerefMut},
        path::PathBuf,
        rc::Rc,
        sync::{
            atomic::{AtomicU8, Ordering as AtomicOrdering},
            mpsc as sync_mpsc, Arc,
        },
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
pub const SET_CURRENT_ADDR: &str = "/rnbo/inst/control/sets/current/name";
pub const SET_PRESETS_LOAD_ADDR: &str = "/rnbo/inst/control/sets/presets/load";
pub const SET_PRESETS_LOADED_ADDR: &str = "/rnbo/inst/control/sets/presets/loaded";

const VOLUME_WHEEL_ENCODER: usize = 9;
const JOG_WHEEL_ENCODER: usize = 10;

const PARAM_PAGE_SIZE: usize = 8;

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

fn brightness_sysex(level: u8) -> [Midi; 3] {
    [
        Midi::new(&[0xF0, 0x00, 0x21]),
        Midi::new(&[0x1D, 0x01, 0x01]),
        Midi::new(&[0x06, level.max(127) as u8, 0xF7]),
    ]
}

fn led_color(index: u8, color: &Srgb<u8>) -> [Midi; 6] {
    let (mut r, mut g, mut b) = color.into_components();

    //need at least 1 bit set
    r = r.max(1);
    g = g.max(1);
    b = b.max(1);

    let chan = 0b0001_0000; /*cc*/
    let index = index + 71;

    //println!("led_color({}, {}, {}, {}, {})", chan, index, r, g, b);

    //let chan = 0b0000_0000; /*note*/
    [
        Midi::new(&[0xF0, 0x00, 0x21]),
        Midi::new(&[0x1D, 0x01, 0x01]),
        Midi::new(&[0x3b, chan, index]),
        Midi::new(&[r & 0x7F, r >> 7, g & 0x7f]),
        Midi::new(&[g >> 7, b & 0x7F, b >> 7]),
        Midi::new(&[0xF7]),
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

#[derive(Copy, Clone, Debug, PartialEq)]
struct ParamUpdate {
    instance: usize,
    index: usize,
}

#[derive(Clone, Debug, PartialEq)]
enum Events {
    BtnDown(Button),
    EncLeft(usize),
    EncRight(usize),
    EncTouch(usize),

    ParamUpdate(ParamUpdate),
    Transport(bool),
    Tempo(f32),

    SetNamesChanged,
    SetPresetNamesChanged,

    SetCurrentChanged,
    SetPresetLoadedChanged,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PatcherParams {
    index: usize, //not instance index, index within out list
    page: usize,
    focused: Option<usize>,
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
        SetsList(usize) + BtnDown(Button::JogWheel) / ctx.set_select(*state).await; = SetsList(*state),
        //SetsList(usize) + EncRight(JOG_WHEEL_ENCODER) [ctx.sets_len() == 0] = Menu(MenuItems::Sets), //abort
        SetsList(usize) + SetNamesChanged = Menu(SETS_INDEX), //backout, TODO be smarter
        SetsList(usize) + SetCurrentChanged = SetsList(*state), //redraw

        SetPresetsList(usize) + BtnDown(Button::Back) = Menu(SET_PRESETS_INDEX),
        SetPresetsList(usize) + EncRight(JOG_WHEEL_ENCODER) [ctx.set_presets_len() > *state + 1] = SetPresetsList(*state + 1),
        SetPresetsList(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = SetPresetsList(*state - 1),
        SetPresetsList(usize) + BtnDown(Button::JogWheel) / ctx.set_preset_select(*state).await;,
        SetPresetsList(usize) + SetPresetNamesChanged = Menu(SET_PRESETS_INDEX), //back out TODO be smarter
        SetPresetsList(usize) + SetPresetLoadedChanged = SetPresetsList(*state), //redraw

        PatcherInstances(usize) + BtnDown(Button::Back) = Menu(PATCHER_INSTANCES_INDEX),
        PatcherInstances(usize) + EncRight(JOG_WHEEL_ENCODER) [ctx.patcher_instances_len() > *state + 1] = PatcherInstances(*state + 1),
        PatcherInstances(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = PatcherInstances(*state - 1),
        PatcherInstances(usize) + BtnDown(Button::JogWheel) / ctx.render_param_page(*state, 0);
            = PatcherParams(PatcherParams { index: *state, page: 0, focused: None }),

        PatcherParams(PatcherParams) + BtnDown(Button::Back) / ctx.clear_params(); = PatcherInstances(state.index),
        PatcherParams(PatcherParams) + EncRight(JOG_WHEEL_ENCODER) [ctx.patcher_instance_param_pages(state.index) > state.page + 1] / ctx.render_param_page(state.index, state.page + 1);
            = PatcherParams(PatcherParams { index: state.index, page: state.page + 1, focused: state.focused }),
        PatcherParams(PatcherParams) + EncLeft(JOG_WHEEL_ENCODER) [state.page > 0] / ctx.render_param_page(state.index, state.page - 1);
            = PatcherParams(PatcherParams { index: state.index, page: state.page - 1, focused: state.focused }),
        PatcherParams(PatcherParams) + EncTouch(_) [*event < 8]
            = PatcherParams(PatcherParams { index: state.index, page: state.page, focused: Some(*event) }),
        PatcherParams(PatcherParams) + EncLeft(_) [*event < 8] / ctx.offset_param(state.index, state.page, *event, -1).await;,
        PatcherParams(PatcherParams) + EncRight(_) [*event < 8] / ctx.offset_param(state.index, state.page, *event, 1).await;,

        //draw param and update the LED if it is in view
        //this actually updates the state and we might not need to, but we do need to render the
        //param
        PatcherParams(PatcherParams) + ParamUpdate(_) [ctx.param_visible(event, state)] / ctx.render_param(event.instance, event.index); = PatcherParams(state.clone()),

        _ + EncRight(VOLUME_WHEEL_ENCODER) / ctx.offset_volume(1);,
        _ + EncLeft(VOLUME_WHEEL_ENCODER) / ctx.offset_volume(-1);,

        _ + BtnDown(Button::PowerShort) / ctx.send_power_cmd(PowerCommand::ClearShortPress); = PromptPower,
        _ + BtnDown(Button::PowerLong) / ctx.send_power_cmd(PowerCommand::ClearLongPress); = PowerOff,
        _ + Tempo(_) / ctx.update_tempo(*event);,
        _ + Transport(_) / ctx.update_transport(*event);,
        _ + BtnDown(Button::Play) / ctx.toggle_transport().await;,
    }
}

pub struct StateController {
    pub params: HashMap<String, usize>,
    pub params_norm: HashMap<String, usize>,

    set_current_name: Option<String>,
    set_preset_loaded_name: Option<String>,

    set_current_index: Option<usize>,
    set_preset_loaded_index: Option<usize>,

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
    patcher_instance_to_index: HashMap<usize, usize>,
    patcher_params: HashMap<usize, Vec<Param>>,
    set_selected: Option<String>,
    volume: Arc<AtomicU8>,
    config: Config,
    config_path: PathBuf,
}

impl Context {
    fn new(
        midi_out_queue: sync_mpsc::SyncSender<Midi>,
        display: &mut Rc<Mutex<MoveDisplay>>,
        volume: Arc<AtomicU8>,
        config_path: PathBuf,
    ) -> Self {
        //send a reset
        let _ = midi_out_queue.send(Midi::reset());

        for m in brightness_sysex(127) {
            let _ = midi_out_queue.send(m);
        }

        //do config
        let config = if std::path::Path::exists(&config_path) {
            if let Ok(file) = File::open(&config_path) {
                let reader = BufReader::new(file);
                serde_json::from_reader(reader).unwrap_or_default()
            } else {
                Config::default()
            }
        } else {
            Config::default()
        };

        volume.store(config.volume, AtomicOrdering::SeqCst);

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
            patcher_instance_to_index: HashMap::new(),
            patcher_params: HashMap::new(),
            volume,
            config,
            config_path,
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

    fn patcher_instance_param_pages(&self, instance: usize) -> usize {
        self.patcher_params
            .get(&instance)
            .map(|params| {
                params.len() / PARAM_PAGE_SIZE
                    + if params.len() % PARAM_PAGE_SIZE == 0 {
                        0
                    } else {
                        1
                    }
            })
            .unwrap_or(0)
    }

    fn patcher_instance_params(&self, instance: usize) -> usize {
        self.patcher_params
            .get(&instance)
            .map(|params| params.len())
            .unwrap_or(0)
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
        let mut indexes: Vec<usize> = instances.keys().map(|k| *k).collect();
        indexes.sort();

        self.patcher_instance_to_index.clear();
        self.patcher_instance_names.clear();
        self.patcher_params.clear();

        //context addresses instances by index from 0, not by instance index (which could be
        //sparse)
        for (index, i) in indexes.iter().enumerate() {
            self.patcher_instance_to_index.insert(*i, index);

            let inst = instances.get(&i).unwrap();
            let mut params = inst.params().clone();
            params.sort_by(|a, b| a.index().cmp(&b.index()));

            self.patcher_params.insert(index, params);
            self.patcher_instance_names
                .push(format!("{}: {}", i, inst.name()));
        }
    }

    fn patcher_instance_names(&self) -> &Vec<String> {
        &self.patcher_instance_names
    }

    fn light_button(&mut self, btn: u8, val: u8) {
        let _ = self.midi_out_queue.send(Midi::cc(btn, val, 0));
    }

    fn offset_volume(&mut self, amt: isize) {
        let cur = self.config.volume as isize;
        let next = (cur + amt).clamp(0, 255);
        if next != cur {
            self.config.volume = next as u8;
            self.volume
                .store(self.config.volume, AtomicOrdering::SeqCst);
        }
    }

    fn param_visible(&self, update: &ParamUpdate, state: &PatcherParams) -> bool {
        let offset = state.page * PARAM_PAGE_SIZE;
        let range = offset..(offset + PARAM_PAGE_SIZE);
        state.index == update.instance && range.contains(&update.index)
    }

    async fn offset_param(&mut self, instance: usize, page: usize, param: usize, offset: isize) {
        let param = page * PARAM_PAGE_SIZE + param;
        if let Some(instance) = self.patcher_params.get_mut(&instance) {
            if let Some(param) = instance.get_mut(param) {
                let mut args = Vec::new();
                let step = 0.01; //TODO allow for other step sizes
                                 //operate on the normalized value.. TODO, change step
                let v = (param.norm() + if offset > 0 { step } else { -step }).clamp(0.0, 1.0);
                param.set_norm(v);
                args.push(OscType::Double(v));
                let msg = OscMessage {
                    addr: param.addr_norm().to_string(),
                    args,
                };
                self.send_osc(msg).await;
            }
        }
        self.render_param(instance, param);
    }

    fn update_param(&mut self, instance: usize, msg: &OscMessage) -> Option<Events> {
        let instance = self.patcher_instance_to_index.get(&instance)?;

        //TODO get norm from OSC
        if let Some(params) = self.patcher_params.get_mut(&instance) {
            if let Some((index, p)) = params
                .iter_mut()
                .enumerate()
                .find(|(_, p)| p.addr() == msg.addr)
            {
                match &msg.args[0] {
                    OscType::Double(v) => p.update_f64(*v),
                    OscType::Float(v) => p.update_f64(*v as f64),
                    OscType::Int(v) => p.update_f64(*v as f64),
                    OscType::String(v) => p.update_s(v),
                    _ => {
                        return None;
                    }
                }
                return Some(Events::ParamUpdate(ParamUpdate {
                    instance: *instance,
                    index,
                }));
            }
        }
        None
    }

    //return is instance index, param index, norm value
    fn update_param_norm(&mut self, instance: usize, msg: &OscMessage) -> Option<Events> {
        let instance = self.patcher_instance_to_index.get(&instance)?;
        if let Some(params) = self.patcher_params.get_mut(&instance) {
            if let Some((index, p)) = params
                .iter_mut()
                .enumerate()
                .find(|(_, p)| p.addr_norm() == msg.addr)
            {
                let v = match &msg.args[0] {
                    OscType::Double(v) => {
                        p.set_norm_pending(*v);
                        Some(*v)
                    }
                    OscType::Float(v) => {
                        let v = *v as f64;
                        p.set_norm_pending(v);
                        Some(v)
                    }
                    OscType::Int(v) => {
                        let v = *v as f64;
                        p.set_norm_pending(v);
                        Some(v)
                    }
                    _ => None,
                };
                return v.map(|_v| {
                    Events::ParamUpdate(ParamUpdate {
                        instance: *instance,
                        index,
                    })
                });
            }
        }
        None
    }

    fn param(&self, instance: usize, index: usize) -> Option<&Param> {
        if let Some(params) = self.patcher_params.get(&instance) {
            params.get(index)
        } else {
            None
        }
    }

    fn clear_params(&mut self) {
        for index in 0..PARAM_PAGE_SIZE {
            let num = index + 71;
            let _ = self
                .midi_out_queue
                .send(Midi::cc(num as u8, MoveColor::Black as _, 0));
        }
    }

    fn render_param(&mut self, instance: usize, index: usize) {
        let num = (index % PARAM_PAGE_SIZE) as u8;

        if let Some(p) = self.param(instance, index) {
            let cap = 0.96;
            let v = p.norm_prefer_pending();

            //TODO get from metdata?
            let color = Srgb::new(1.0, 1.0, 1.0).darken(cap - v * cap).into_format();

            for m in led_color(num, &color) {
                let _ = self.midi_out_queue.send(m);
            }
        }
    }

    fn render_param_page(&mut self, instance: usize, page: usize) {
        self.clear_params();
        let offset = page * PARAM_PAGE_SIZE;
        for index in 0..PARAM_PAGE_SIZE {
            self.render_param(instance, index + offset);
        }
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
        volume: Arc<AtomicU8>,
        config_path: PathBuf,
    ) -> Self {
        let mut context = Context::new(midi_out_queue, display, volume, config_path);

        context.light_button(MENU_MIDI, MoveColor::LightGray as _);
        context.light_button(PLAY_MIDI, MoveColor::LightGray as _);

        Self {
            params: HashMap::new(),
            params_norm: HashMap::new(),
            sysex: Vec::new(),
            statemachine: StateMachine::new(context),
            set_current_name: None,
            set_preset_loaded_name: None,

            set_current_index: None,
            set_preset_loaded_index: None,
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
        let mut params_norm: HashMap<String, usize> = HashMap::new();

        for (index, v) in instances.iter() {
            for p in v.params().iter() {
                params.insert(p.addr().to_string(), *index);
                params_norm.insert(p.addr_norm().to_string(), *index);
            }
        }
        self.params = params;
        self.params_norm = params_norm;
    }

    pub async fn set_set_names(&mut self, names: &Vec<String>) {
        self.context_mut().set_set_names(names);
        self.handle_event(Events::SetNamesChanged).await;
    }

    pub async fn set_set_preset_names(&mut self, names: &Vec<String>) {
        self.context_mut().set_set_preset_names(names);
        self.handle_event(Events::SetPresetNamesChanged).await;
    }

    pub async fn handle_osc(&mut self, msg: &OscMessage) {
        if msg.args.len() == 1 {
            println!("got osc {}", msg.addr);
            //let mut update = None;
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
                SET_CURRENT_ADDR => {
                    self.set_current_name = match &msg.args[0] {
                        OscType::String(name) => Some(name.clone()),
                        _ => None,
                    };
                    self.set_current_index = if let Some(name) = &self.set_current_name {
                        self.context().set_names().iter().position(|r| r == name)
                    } else {
                        None
                    };
                    self.handle_event(Events::SetCurrentChanged).await;
                }
                SET_PRESETS_LOADED_ADDR => {
                    self.set_preset_loaded_name = match &msg.args[0] {
                        OscType::String(name) => Some(name.clone()),
                        _ => None,
                    };
                    self.set_preset_loaded_index = if let Some(name) = &self.set_preset_loaded_name
                    {
                        self.context()
                            .set_preset_names()
                            .iter()
                            .position(|r| r == name)
                    } else {
                        None
                    };
                    self.handle_event(Events::SetPresetLoadedChanged).await;
                }
                _ => {
                    if let Some(instance) = self.params.get(&msg.addr).map(|i| *i) {
                        if let Some(_e) = self.context_mut().update_param(instance, msg) {
                            //ignore, we wait for normalized
                            //self.handle_event(e).await;
                        }
                    } else if let Some(instance) = self.params_norm.get(&msg.addr).map(|i| *i) {
                        if let Some(e) = self.context_mut().update_param_norm(instance, msg) {
                            self.handle_event(e).await;
                        }
                    }
                }
            }
        }
    }

    pub async fn handle_sysex(&mut self) {
        println!("handle sysex {:02x?}", self.sysex);
        let sysex: Vec<u8> = std::mem::take(&mut self.sysex);
        if sysex.len() >= 6 {
            match sysex[0..6] {
                [0x00, 0x21, 0x1d, 0x01, 0x01, 0x3a] => {
                    //println!("power sysex {:02x?}", sysex);
                    if let Some(status) = sysex.get(6) {
                        if status & 0b1_0000 != 0 {
                            self.handle_event(Events::BtnDown(Button::PowerLong)).await;
                        } else if status & 0b1000 != 0 {
                            self.handle_event(Events::BtnDown(Button::PowerShort)).await;
                        }
                    }
                }
                _ => {
                    println!("unhandled sysex {:02x?}", sysex);
                }
            }
        } else {
            println!("unhandled sysex {:02x?}", sysex);
        }
    }

    pub async fn handle_midi(&mut self, bytes: &[u8]) {
        //println!("got midi {:02x?}", bytes);

        //volume 0x08
        //jog 0x09
        match bytes.len() {
            1 => {
                println!("got 1 byte midi {:?}", bytes);
                if bytes[0] == 0xF7 {
                    self.handle_sysex().await;
                } else if bytes[0] & 0x80 != 0 {
                    self.sysex.clear();
                } else if self.sysex.len() > 0 {
                    self.sysex.extend_from_slice(bytes);
                }
            }
            2 => {
                println!("got 2 byte midi {:?}", bytes);
                if bytes[0] == 0xF7 {
                    self.handle_sysex().await;
                } else if bytes[1] == 0xF7 {
                    self.sysex.push(bytes[0]);
                    self.handle_sysex().await;
                } else if bytes[0] & 0x80 != 0 {
                    self.sysex.clear();
                } else if self.sysex.len() > 0 {
                    self.sysex.extend_from_slice(bytes);
                }
            }
            3 => match bytes[0] {
                0x90 => {
                    self.sysex.clear();
                    if bytes[1] < 10 && bytes[2] != 0 {
                        self.handle_event(Events::EncTouch(bytes[1] as usize)).await;
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
                        0x4f => match bytes[2] {
                            1 => {
                                self.handle_event(Events::EncRight(VOLUME_WHEEL_ENCODER))
                                    .await;
                            }
                            127 => {
                                self.handle_event(Events::EncLeft(VOLUME_WHEEL_ENCODER))
                                    .await;
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

                        //param encoders
                        index @ 71..=78 => {
                            let index = (index - 71) as usize;
                            match bytes[2] {
                                1 => {
                                    self.handle_event(Events::EncRight(index)).await;
                                }
                                127 => {
                                    self.handle_event(Events::EncLeft(index)).await;
                                }
                                _ => (),
                            }
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
                            self.sysex.extend_from_slice(bytes);
                        }
                    }
                }
            },
            _ => {
                println!("got other byte midi {:?}", bytes);
            }
        }
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
                        draw_menu(display, &"RNBO On Move", &MENU_ITEMS, selected, None);
                    }
                    let ctx = self.context_mut();
                    ctx.light_button(MENU_MIDI, 0);
                    ctx.light_button(BACK_MIDI, 0);
                }
                States::SetsList(selected) => {
                    let selected = *selected;
                    {
                        let display = self.locked_display().await;
                        let indicated = self.set_current_index;
                        draw_menu(
                            display,
                            &"Load Set",
                            self.context().set_names(),
                            selected,
                            indicated,
                        );
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
                        let indicated = self.set_preset_loaded_index;
                        draw_menu(
                            display,
                            &"Load Set Preset",
                            self.context().set_preset_names(),
                            selected,
                            indicated,
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
                            None,
                        );
                    }

                    self.context_mut()
                        .light_button(MENU_MIDI, MoveColor::Black as _);
                    self.context_mut()
                        .light_button(BACK_MIDI, MoveColor::LightGray as _);
                }
                States::PatcherParams(state) => {
                    let index = state.index;
                    let page = state.page;
                    let focus = state.focused.clone();
                    {
                        let pages = self.context().patcher_instance_param_pages(index);

                        //focused valaue
                        let focus = if let Some(focus) = focus {
                            if let Some(param) =
                                self.context().param(index, focus + page * PARAM_PAGE_SIZE)
                            {
                                Some(format!("{}\n{}", param.name(), param.render_value()))
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        let text_style =
                            MonoTextStyle::new(&profont::PROFONT_12_POINT, BinaryColor::On);
                        let name = self.context().patcher_instance_names.get(index).unwrap();

                        let mut title = format!("{} Params", name);
                        if title.len() > 16 {
                            title.truncate(14);
                            title.push_str("..");
                        }

                        let mut display = self.locked_display().await;
                        display.clear(BinaryColor::Off).unwrap();

                        Text::with_alignment(
                            title.as_str(),
                            Point::new(DISPLAY_WIDTH as i32 / 2, 11),
                            text_style,
                            Alignment::Center,
                        )
                        .draw(display.deref_mut())
                        .unwrap();

                        //draw pager
                        if pages > 1 {
                            let style = PrimitiveStyleBuilder::new()
                                .stroke_color(BinaryColor::On)
                                .stroke_width(1)
                                .fill_color(BinaryColor::On)
                                .build();

                            let step = DISPLAY_WIDTH / pages as u32;
                            let width = step - 4;

                            let y = (DISPLAY_HEIGHT - 3) as i32;
                            let mut x = (step / 2) as i32;

                            //TODO assert that we can actually draw these

                            for p in 0..pages {
                                let height = if p == page { 3 } else { 1 };
                                Rectangle::with_center(Point::new(x, y), Size::new(width, height))
                                    .into_styled(style)
                                    .draw(display.deref_mut())
                                    .unwrap();
                                x = x + (step as i32);
                            }
                        }

                        if let Some(focus) = focus {
                            Text::with_alignment(
                                focus.as_str(),
                                Point::new(DISPLAY_WIDTH as i32 / 2, DISPLAY_HEIGHT as i32 / 2),
                                text_style,
                                Alignment::Center,
                            )
                            .draw(display.deref_mut())
                            .unwrap();
                        }
                    }
                    let ctx = self.context_mut();
                    ctx.light_button(MENU_MIDI, MoveColor::Black as _);
                    ctx.light_button(BACK_MIDI, MoveColor::LightGray as _);
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
    indicated: Option<usize>,
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
        let off = index + start;
        let indicator = if Some(off) == indicated { &"*" } else { &" " };

        *l = if off == selected {
            format!(">{}{}", indicator, item.as_ref())
        } else {
            format!(" {}{}", indicator, item.as_ref())
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

impl Drop for Context {
    fn drop(&mut self) {
        if let Ok(file) = std::fs::File::create(&self.config_path) {
            let _ = serde_json::to_writer_pretty(file, &self.config);
        }
    }
}
