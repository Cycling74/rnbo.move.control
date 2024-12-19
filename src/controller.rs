use {
    crate::{
        config::Config,
        display::{MoveDisplay, DISPLAY_HEIGHT, DISPLAY_WIDTH},
        midi::Midi,
        param::Param,
        patcher::PatcherInst,
        view::ParamView,
    },
    embedded_graphics::{
        mono_font::MonoTextStyle,
        pixelcolor::BinaryColor,
        prelude::*,
        primitives::{PrimitiveStyleBuilder, Rectangle},
        text::{Alignment, Text},
    },
    futures_util::{stream::SplitSink, SinkExt},
    palette::{Darken, Srgb},
    reqwest_websocket::{Message, WebSocket},
    rosc::{OscMessage, OscPacket, OscType},
    std::{
        cmp::PartialEq,
        collections::HashMap,
        fs::File,
        io::BufReader,
        ops::DerefMut,
        path::PathBuf,
        rc::Rc,
        sync::{
            atomic::{AtomicU8, Ordering as AtomicOrdering},
            mpsc as sync_mpsc, Arc,
        },
        time::Duration,
    },
    tokio::sync::{Mutex, MutexGuard},
};

const MENU_MIDI: u8 = 0x32;
const BACK_MIDI: u8 = 0x33;
const PLAY_MIDI: u8 = 0x55;

const MOVE_CTL_MIDI_CHAN: u8 = 15;

const TRANSPORT_ROLLING_ADDR: &str = "/rnbo/jack/transport/rolling";
const TRANSPORT_BPM_ADDR: &str = "/rnbo/jack/transport/bpm";

const TITLE_TEXT_STYLE: MonoTextStyle<BinaryColor> =
    MonoTextStyle::new(&profont::PROFONT_12_POINT, BinaryColor::On);

pub const INST_UNLOAD_ADDR: &str = "/rnbo/inst/control/unload";
pub const SET_LOAD_ADDR: &str = "/rnbo/inst/control/sets/load";
pub const SET_CURRENT_ADDR: &str = "/rnbo/inst/control/sets/current/name";
pub const SET_PRESETS_LOAD_ADDR: &str = "/rnbo/inst/control/sets/presets/load";
pub const SET_PRESETS_LOADED_ADDR: &str = "/rnbo/inst/control/sets/presets/loaded";

const VOLUME_WHEEL_BUTTON: usize = 8;
const VOLUME_WHEEL_ENCODER: usize = 9;
const JOG_WHEEL_ENCODER: usize = 10;

const PARAM_PAGE_SIZE: usize = 8;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ExitCmd {
    Exit,
    LaunchMove,
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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

fn _brightness_sysex(level: u8) -> [Midi; 3] {
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
    PowerLong,
    PowerShort,
    Menu,
    Play,
}

#[derive(Copy, Clone, Debug, PartialEq)]
struct ParamUpdate {
    instance: usize, //local index
    index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Events {
    BtnDown(Button),
    BtnUp(Button),
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

const MENU_ITEMS: [&'static str; 4] = ["Set Presets", "Sets", "Patcher Instances", "Tempo"];
const EXIT_MENU: [&'static str; 2] = ["Power Down", "Launch Move"];

const SET_PRESETS_INDEX: usize = 0;
const SETS_INDEX: usize = 1;
const PATCHER_INSTANCES_INDEX: usize = 2;
const TEMPO_INDEX: usize = 3;

#[derive(Clone, Debug, PartialEq)]
enum Cmd {
    Power(PowerCommand),

    OffsetParam {
        instance: usize,
        index: usize,
        offset: isize,
    },
    OffsetVolume(isize),
    OffsetTempo(isize),
    MulTempoOffset(bool),

    ToggleTransport,

    LightButton {
        btn: u8,
        val: u8,
    },

    RenderParamPage {
        instance: usize,
        page: usize,
    },

    RenderParam {
        instance: usize,
        param: usize,
    },

    LoadSet(usize),
    LoadSetPreset(usize),

    ClearParams,
}

mod top {
    use super::{
        Button, Cmd, Context, Events, PowerCommand, EXIT_MENU, JOG_WHEEL_ENCODER,
        VOLUME_WHEEL_BUTTON, VOLUME_WHEEL_ENCODER,
    };

    const POWER_DOWN_INDEX: usize = 0;
    const LAUNCH_MOVE_INDEX: usize = 1;

    smlang::statemachine! {
        states_attr: #[derive(Clone, Debug)],
        transitions: {
            *Init + BtnDown(Button::Menu) = Main,
            Init + BtnDown(Button::JogWheel) = Main,
            Init + BtnDown(Button::Back) = Main,

            VolumeEditor + BtnDown(Button::PowerShort) / ctx.emit(Cmd::Power(PowerCommand::ClearShortPress)); = PromptExit(POWER_DOWN_INDEX),

            Main + EncTouch(VOLUME_WHEEL_BUTTON) = VolumeEditor,
            Main + EncRight(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(1)); = VolumeEditor,
            Main + EncLeft(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(-1)); = VolumeEditor,

            VolumeEditor + BtnDown(Button::Back) = Main,
            VolumeEditor + BtnDown(Button::Menu) = Main,
            VolumeEditor + EncRight(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(1)); = VolumeEditor,
            VolumeEditor + EncLeft(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(-1)); = VolumeEditor,
            VolumeEditor + EncTouch(_) [*event != VOLUME_WHEEL_BUTTON] = Main,

            PromptExit(usize) + BtnDown(Button::JogWheel) [*state == POWER_DOWN_INDEX] = PowerOff,
            PromptExit(usize) + BtnDown(Button::JogWheel) [*state == LAUNCH_MOVE_INDEX] = LaunchMove,
            PromptExit(usize) + EncRight(JOG_WHEEL_ENCODER) [*state + 1 < EXIT_MENU.len()] = PromptExit(*state + 1),
            PromptExit(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = PromptExit(*state - 1),
            PromptExit(usize) + BtnDown(Button::Back) = Main,
            PromptExit(usize) + BtnDown(Button::Menu) = Main,

            _ + BtnDown(Button::PowerShort) / ctx.emit(Cmd::Power(PowerCommand::ClearShortPress)); = PromptExit(POWER_DOWN_INDEX),
            _ + BtnDown(Button::PowerLong) / ctx.emit(Cmd::Power(PowerCommand::ClearLongPress)); = PowerOff,

            _ + Tempo(_),
            _ + Transport(_),
            _ + BtnDown(Button::Play) / ctx.emit(Cmd::ToggleTransport);,

        }
    }
}

smlang::statemachine! {
    states_attr: #[derive(Clone, Debug)],
    transitions: {
        *Init + BtnDown(Button::Back) = Init, //dummy

        //nav
        Menu(usize) + EncRight(JOG_WHEEL_ENCODER) [*state + 1 < MENU_ITEMS.len()] = Menu(*state + 1),
        Menu(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = Menu(*state - 1),

        //select
        Menu(usize) + BtnDown(Button::JogWheel) [*state == SETS_INDEX && ctx.sets_count() > 0] = SetsList(0),
        Menu(usize) + BtnDown(Button::JogWheel) [*state == SET_PRESETS_INDEX && ctx.set_presets_count() > 0] = SetPresetsList(0),
        //skip patcher instances menu if there is only 1 instance
        Menu(usize) + BtnDown(Button::JogWheel) [*state == PATCHER_INSTANCES_INDEX && ctx.instances_count() > 1] = PatcherInstances(0),
        Menu(usize) + BtnDown(Button::JogWheel) [*state == PATCHER_INSTANCES_INDEX && ctx.instances_count() == 1] / ctx.emit(Cmd::RenderParamPage{instance: 0, page: 0});
            = PatcherParams(PatcherParams { index: 0, page: 0, focused: None }),

        Menu(usize) + BtnDown(Button::JogWheel) [*state == TEMPO_INDEX] = TempoEditor,

        SetsList(usize) + BtnDown(Button::Back) = Menu(SETS_INDEX),
        SetsList(usize) + EncRight(JOG_WHEEL_ENCODER) [ctx.sets_count() > *state + 1] = SetsList(*state + 1),
        SetsList(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = SetsList(*state - 1),
        SetsList(usize) + BtnDown(Button::JogWheel) / ctx.emit(Cmd::LoadSet(*state)); = SetsList(*state),
        SetsList(usize) + SetNamesChanged = Menu(SETS_INDEX), //backout, TODO be smarter
        SetsList(usize) + SetCurrentChanged = SetsList(*state), //redraw

        SetPresetsList(usize) + BtnDown(Button::Back) = Menu(SET_PRESETS_INDEX),
        SetPresetsList(usize) + EncRight(JOG_WHEEL_ENCODER) [ctx.set_presets_count() > *state + 1] = SetPresetsList(*state + 1),
        SetPresetsList(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = SetPresetsList(*state - 1),
        SetPresetsList(usize) + BtnDown(Button::JogWheel) / ctx.emit(Cmd::LoadSetPreset(*state));,
        SetPresetsList(usize) + SetPresetNamesChanged = Menu(SET_PRESETS_INDEX), //back out TODO be smarter
        SetPresetsList(usize) + SetPresetLoadedChanged = SetPresetsList(*state), //redraw

        PatcherInstances(usize) + BtnDown(Button::Back) = Menu(PATCHER_INSTANCES_INDEX),
        PatcherInstances(usize) + EncRight(JOG_WHEEL_ENCODER) [ctx.instances_count() > *state + 1] = PatcherInstances(*state + 1),
        PatcherInstances(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = PatcherInstances(*state - 1),
        PatcherInstances(usize) + BtnDown(Button::JogWheel) / ctx.emit(Cmd::RenderParamPage{ instance: *state, page: 0});
            = PatcherParams(PatcherParams { index: *state, page: 0, focused: None }),

        //skip patcher instances menu if there is only 1 instance
        PatcherParams(PatcherParams) + BtnDown(Button::Back) [ctx.instances_count() > 1] / ctx.emit(Cmd::ClearParams); = PatcherInstances(state.index),
        PatcherParams(PatcherParams) + BtnDown(Button::Back) [ctx.instances_count() == 1] / ctx.emit(Cmd::ClearParams); = Menu(PATCHER_INSTANCES_INDEX),

        PatcherParams(PatcherParams) + EncRight(JOG_WHEEL_ENCODER) [ctx.instance_param_pages(state.index) > state.page + 1] / ctx.emit(Cmd::RenderParamPage { instance: state.index, page: state.page + 1});
            = PatcherParams(PatcherParams { index: state.index, page: state.page + 1, focused: state.focused }),
        PatcherParams(PatcherParams) + EncLeft(JOG_WHEEL_ENCODER) [state.page > 0] / ctx.emit(Cmd::RenderParamPage{ instance: state.index, page: state.page - 1});
            = PatcherParams(PatcherParams { index: state.index, page: state.page - 1, focused: state.focused }),
        PatcherParams(PatcherParams) + EncTouch(_) [*event < 8]
            = PatcherParams(PatcherParams { index: state.index, page: state.page, focused: Some(*event) }),
        PatcherParams(PatcherParams) + EncLeft(_) [*event < 8] / ctx.emit(Cmd::OffsetParam { instance: state.index, index: state.page * PARAM_PAGE_SIZE + *event, offset: -1});,
        PatcherParams(PatcherParams) + EncRight(_) [*event < 8] / ctx.emit(Cmd::OffsetParam { instance: state.index, index: state.page * PARAM_PAGE_SIZE + *event, offset: 1});,

        PatcherInstances(usize) + SetCurrentChanged = Menu(PATCHER_INSTANCES_INDEX),
        PatcherParams(PatcherParams) + SetCurrentChanged  / ctx.emit(Cmd::ClearParams); = Menu(PATCHER_INSTANCES_INDEX),

        PatcherParams(PatcherParams) + ParamUpdate(_) [ param_visible(event, state) ] / ctx.emit(Cmd::RenderParam { instance: event.instance, param: event.index }); = PatcherParams(state.clone()),

        TempoEditor + BtnDown(Button::Back) = Menu(TEMPO_INDEX),
        TempoEditor + EncRight(JOG_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetTempo(1)); = TempoEditor,
        TempoEditor + EncLeft(JOG_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetTempo(-1)); = TempoEditor,
        TempoEditor + BtnDown(Button::JogWheel) / ctx.emit(Cmd::MulTempoOffset(true)); = TempoEditor,
        TempoEditor + BtnUp(Button::JogWheel) / ctx.emit(Cmd::MulTempoOffset(false));  = TempoEditor,
        TempoEditor + Tempo(_) = TempoEditor,

        _ + BtnDown(Button::Menu) / ctx.emit(Cmd::ClearParams); = Menu(0),
    }
}

pub struct StateController {
    set_current_name: Option<String>,
    set_preset_loaded_name: Option<String>,

    set_current_index: Option<usize>,
    set_preset_loaded_index: Option<usize>,

    sysex: Vec<u8>,

    exit_cmd: Option<ExitCmd>,

    topsm: top::StateMachine,
    sm: StateMachine,

    cmd_queue: sync_mpsc::Receiver<Cmd>,

    ws_tx: Option<SplitSink<WebSocket, Message>>,
    midi_out_queue: sync_mpsc::SyncSender<Midi>,
    display: Rc<Mutex<MoveDisplay>>,
    volume: Arc<AtomicU8>,

    config: Config,
    config_path: PathBuf,

    rolling: bool,
    bpm: f32,
    tempo_offset_mul: f32,

    params: Vec<Param>,

    instance_params: Vec<Vec<usize>>,

    //(sparce instance index, param_index) -> (local instance_index, param index)
    instance_param_map: HashMap<(usize, usize), (usize, usize)>,

    param_lookup: HashMap<String, usize>, //OSC addr -> index into self.params
    param_norm_lookup: HashMap<String, usize>, //OSC addr -> index into self.params

    param_views: Vec<ParamView>,

    set_names: Vec<String>,
    set_preset_names: Vec<String>,
    patcher_instance_names: Vec<String>,
}

#[derive(Clone, Debug)]
struct CommonContext {
    pub(crate) sets_count: usize,
    pub(crate) set_presets_count: usize,
    pub(crate) instances_count: usize,

    //sorted list of instances that have params, and the count of pages
    pub(crate) instance_param_pages: Vec<usize>,
    pub(crate) view_param_pages: Vec<usize>,
}

impl Default for CommonContext {
    fn default() -> Self {
        Self {
            sets_count: 0,
            set_presets_count: 0,
            instances_count: 0,

            instance_param_pages: Vec::new(),
            view_param_pages: Vec::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Context {
    cmd_queue: sync_mpsc::Sender<Cmd>,
    common: CommonContext,
}

fn param_visible(update: &ParamUpdate, state: &PatcherParams) -> bool {
    let offset = state.page * PARAM_PAGE_SIZE;
    let range = offset..(offset + PARAM_PAGE_SIZE);
    state.index == update.instance && range.contains(&update.index)
}

impl Context {
    fn new(cmd_queue: super::sync_mpsc::Sender<Cmd>) -> Self {
        Self {
            cmd_queue,
            common: Default::default(),
        }
    }

    fn emit(&mut self, cmd: Cmd) {
        let _ = self.cmd_queue.send(cmd);
    }

    fn common(&self) -> CommonContext {
        self.common.clone()
    }

    fn update_common(&mut self, common: CommonContext) {
        self.common = common;
    }

    fn sets_count(&self) -> usize {
        self.common.sets_count
    }

    fn set_presets_count(&self) -> usize {
        self.common.set_presets_count
    }

    fn instances_count(&self) -> usize {
        self.common.instances_count
    }

    fn instance_param_pages(&self, instance: usize) -> usize {
        *self.common.instance_param_pages.get(instance).unwrap_or(&0)
    }

    fn view_param_pages(&self, view: usize) -> usize {
        *self.common.view_param_pages.get(view).unwrap_or(&0)
    }
}

impl StateController {
    pub fn new(
        midi_out_queue: sync_mpsc::SyncSender<Midi>,
        display: Rc<Mutex<MoveDisplay>>,
        volume: Arc<AtomicU8>,
        config_path: PathBuf,
    ) -> Self {
        let (tx, rx) = sync_mpsc::channel();

        let context = Context::new(tx.clone());
        let sm = StateMachine::new_with_state(context, States::Menu(0));

        let context = Context::new(tx.clone());
        let topsm = top::StateMachine::new(context);

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

        //init volume
        volume.store(config.volume, AtomicOrdering::SeqCst);

        //reset
        let _ = midi_out_queue.send(Midi::reset());

        let mut s = Self {
            sysex: Vec::new(),

            sm,
            topsm,

            midi_out_queue,
            display,
            volume,

            config,
            config_path,

            rolling: false,
            bpm: 100.0,
            tempo_offset_mul: 1.0,

            exit_cmd: None,

            set_current_name: None,
            set_preset_loaded_name: None,

            set_current_index: None,
            set_preset_loaded_index: None,

            cmd_queue: rx,

            ws_tx: None,

            params: Vec::new(),

            instance_params: Vec::new(),
            instance_param_map: HashMap::new(),

            param_lookup: HashMap::new(),
            param_norm_lookup: HashMap::new(),

            param_views: Vec::new(),

            set_names: Vec::new(),
            set_preset_names: Vec::new(),
            patcher_instance_names: Vec::new(),
        };

        //States::Init not transitioned to so, do setup here
        s.light_button(MENU_MIDI, MoveColor::LightGray as _);
        s.light_button(PLAY_MIDI, MoveColor::LightGray as _);

        s
    }

    pub async fn set_ws(&mut self, mut ws: SplitSink<WebSocket, Message>) {
        //query values
        for addr in [TRANSPORT_ROLLING_ADDR, TRANSPORT_BPM_ADDR, SET_CURRENT_ADDR] {
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

    pub fn set_instances(&mut self, instances: HashMap<usize, PatcherInst>) {
        let mut indexes: Vec<usize> = instances.keys().map(|k| *k).collect();
        indexes.sort();

        self.patcher_instance_names.clear();
        self.params.clear();
        self.instance_params.clear();
        self.instance_param_map.clear();
        self.param_lookup.clear();
        self.param_norm_lookup.clear();

        let mut common = self.sm.context().common();
        common.instance_param_pages.clear();

        for (local_instance_index, key) in indexes.iter().enumerate() {
            let inst = instances.get(key).unwrap();

            //XXX what if there aren't any params?
            self.patcher_instance_names
                .push(format!("{}: {}", inst.index(), inst.name()));
            common
                .instance_param_pages
                .push(1 + inst.params().len() / PARAM_PAGE_SIZE);

            let mut instindexes = Vec::new();
            for p in inst.params().iter() {
                let index = self.params.len();
                let local_param_index = instindexes.len();

                self.params.push(p.clone());
                instindexes.push(index);

                //setup maps
                self.param_lookup.insert(p.addr().to_string(), index);
                self.param_norm_lookup
                    .insert(p.addr_norm().to_string(), index);
                self.instance_param_map.insert(
                    (p.instance_index(), p.index()),
                    (local_instance_index, local_param_index),
                );
            }
            self.instance_params.push(instindexes);
        }

        common.instances_count = self.patcher_instance_names.len();
        self.update_common(common);
    }

    pub async fn set_set_current_name(&mut self, name: Option<String>) {
        self.set_current_name = name;
        self.set_current_index = if let Some(name) = &self.set_current_name {
            self.set_names.iter().position(|r| r == name)
        } else {
            None
        };
        self.handle_event(Events::SetCurrentChanged).await;
    }

    pub async fn set_set_names(&mut self, names: &Vec<String>) {
        self.set_names = names.clone();
        self.set_names.sort();
        self.set_names.insert(0, "<empty>".to_string());

        //TODO check set_current_name

        let mut common = self.sm.context().common();
        common.sets_count = self.set_names.len();
        self.update_common(common);

        self.handle_event(Events::SetNamesChanged).await;
    }

    pub async fn set_set_preset_names(&mut self, names: &Vec<String>) {
        self.set_preset_names = names.clone();

        let mut common = self.sm.context().common();
        common.set_presets_count = names.len();
        self.update_common(common);

        self.handle_event(Events::SetPresetNamesChanged).await;
    }

    pub async fn handle_osc(&mut self, msg: &OscMessage) {
        if msg.args.len() == 1 {
            //println!("got osc {}", msg.addr);
            //let mut update = None;
            match msg.addr.as_str() {
                TRANSPORT_ROLLING_ADDR => {
                    if let OscType::Bool(rolling) = msg.args[0] {
                        self.rolling = rolling;
                        self.handle_event(Events::Transport(rolling)).await;
                    }
                }
                TRANSPORT_BPM_ADDR => {
                    if let Some(bpm) = match &msg.args[0] {
                        OscType::Double(v) => Some(*v as f32),
                        OscType::Float(v) => Some(*v),
                        _ => None,
                    } {
                        self.bpm = bpm;
                        self.handle_event(Events::Tempo(bpm)).await;
                    }
                }
                SET_CURRENT_ADDR => {
                    let name = match &msg.args[0] {
                        OscType::String(name) => Some(name.clone()),
                        _ => None,
                    };
                    self.set_set_current_name(name).await;
                }
                SET_PRESETS_LOADED_ADDR => {
                    self.set_preset_loaded_name = match &msg.args[0] {
                        OscType::String(name) => Some(name.clone()),
                        _ => None,
                    };
                    self.set_preset_loaded_index = if let Some(name) = &self.set_preset_loaded_name
                    {
                        self.set_preset_names.iter().position(|r| r == name)
                    } else {
                        None
                    };
                    self.handle_event(Events::SetPresetLoadedChanged).await;
                }
                _ => {
                    if let Some(index) = self.param_lookup.get(&msg.addr) {
                        if let Some(param) = self.params.get_mut(*index) {
                            //ignore, we wait for normalized
                            match &msg.args[0] {
                                OscType::Double(v) => param.update_f64(*v),
                                OscType::Float(v) => param.update_f64(*v as f64),
                                OscType::Int(v) => param.update_f64(*v as f64),
                                OscType::String(v) => param.update_s(v),
                                _ => (),
                            };
                        }
                    } else if let Some(index) = self.param_norm_lookup.get(&msg.addr) {
                        if let Some(param) = self.params.get_mut(*index) {
                            let v = match &msg.args[0] {
                                OscType::Double(v) => {
                                    param.set_norm_pending(*v);
                                    Some((param.instance_index(), param.index()))
                                }
                                OscType::Float(v) => {
                                    let v = *v as f64;
                                    param.set_norm_pending(v);
                                    Some((param.instance_index(), param.index()))
                                }
                                _ => None,
                            };
                            if let Some(sparce) = v {
                                //convert to local indexes
                                if let Some(local) = self.instance_param_map.get(&sparce) {
                                    self.handle_event(Events::ParamUpdate(ParamUpdate {
                                        instance: local.0,
                                        index: local.1,
                                    }))
                                    .await;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pub async fn handle_sysex(&mut self) {
        //println!("handle sysex {:02x?}", self.sysex);
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

    pub async fn handle_midi(&mut self, bytes: &[u8]) -> Option<ExitCmd> {
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
                0x9F => {
                    self.sysex.clear();
                    //0..7 params
                    //8 volume
                    //9 jog wheel
                    if bytes[1] < 10 && bytes[2] != 0 {
                        self.handle_event(Events::EncTouch(bytes[1] as usize)).await;
                    }
                }
                0xBF => {
                    self.sysex.clear();
                    match bytes[1] {
                        //jog wheel btn
                        0x03 => {
                            if bytes[2] != 0 {
                                self.handle_event(Events::BtnDown(Button::JogWheel)).await;
                            } else {
                                self.handle_event(Events::BtnUp(Button::JogWheel)).await;
                            }
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
        };
        self.exit_cmd
    }

    async fn display_centered(&mut self, text: &str) {
        self.with_display(|mut display| {
            let style = MonoTextStyle::new(&profont::PROFONT_12_POINT, BinaryColor::On);
            display.clear(BinaryColor::Off).unwrap();

            draw_centered(&mut display, text, style);
        })
        .await;
    }

    fn update_common(&mut self, common: CommonContext) {
        self.sm.context_mut().update_common(common.clone());
        self.topsm.context_mut().update_common(common);
    }

    fn light_button(&mut self, btn: u8, val: u8) {
        let _ = self
            .midi_out_queue
            .send(Midi::cc(btn, val, MOVE_CTL_MIDI_CHAN));
    }

    fn send_power_cmd(&mut self, cmd: PowerCommand) {
        for m in power_sysex(cmd).into_iter() {
            let _ = self.midi_out_queue.send(m);
        }
    }

    fn volume(&self) -> f32 {
        self.config.volume as f32 / 255.0
    }

    async fn render_state(&mut self, s: &States) {
        match s {
            States::Menu(selected) => {
                let selected: usize = *selected;
                self.with_display(|display| {
                    draw_menu(display, &"RNBO On Move", &MENU_ITEMS, selected, None);
                })
                .await;
                self.light_button(BACK_MIDI, 0);
            }
            States::TempoEditor => {
                self.light_button(BACK_MIDI, MoveColor::LightGray as _);
                self.with_display(|mut display| {
                    display.clear(BinaryColor::Off).unwrap();
                    draw_title(&mut display, &"Tempo (bpm)");
                    let bpm = format!("{:.1}", self.bpm);
                    draw_centered(&mut display, bpm.as_str(), TITLE_TEXT_STYLE);
                })
                .await;
            }
            States::SetsList(selected) => {
                let selected = *selected;
                let indicated = self.set_current_index;
                self.with_display(|display| {
                    draw_menu(
                        display,
                        &"Load Set",
                        self.set_names.as_slice(),
                        selected,
                        indicated,
                    );
                })
                .await;

                self.light_button(BACK_MIDI, MoveColor::LightGray as _);
            }
            States::SetPresetsList(selected) => {
                let selected = *selected;
                let indicated = self.set_preset_loaded_index;
                self.with_display(|display| {
                    draw_menu(
                        display,
                        &"Load Set Preset",
                        self.set_preset_names.as_slice(),
                        selected,
                        indicated,
                    );
                })
                .await;

                self.light_button(BACK_MIDI, MoveColor::LightGray as _);
            }
            States::PatcherInstances(selected) => {
                let selected = *selected;
                self.with_display(|display| {
                    draw_menu(
                        display,
                        &"Patcher Instances",
                        self.patcher_instance_names.as_slice(),
                        selected,
                        None,
                    );
                })
                .await;

                self.light_button(BACK_MIDI, MoveColor::LightGray as _);
            }
            States::PatcherParams(state) => {
                let index = state.index;
                let page = state.page;
                let focused = state.focused.clone();
                {
                    let pages = self.context().instance_param_pages(index);

                    let mut focus: Option<String> = None;
                    if let Some(focused) = focused {
                        if let Some(instance) = self.instance_params.get(index) {
                            let pindex = page * PARAM_PAGE_SIZE + focused;
                            if let Some(pindex) = instance.get(pindex) {
                                if let Some(param) = self.params.get(*pindex) {
                                    focus =
                                        Some(format!("{}\n{}", param.name(), param.render_value()))
                                }
                            }
                        }
                    }

                    let text_style =
                        MonoTextStyle::new(&profont::PROFONT_12_POINT, BinaryColor::On);
                    let name = self.patcher_instance_names.get(index).unwrap();

                    let mut title = format!("{} Params", name);
                    if title.len() > 16 {
                        title.truncate(14);
                        title.push_str("..");
                    }

                    self.with_display(|mut display| {
                        display.clear(BinaryColor::Off).unwrap();

                        draw_title(&mut display, title.as_str());

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

                        if let Some(focus) = &focus {
                            Text::with_alignment(
                                focus.as_str(),
                                Point::new(DISPLAY_WIDTH as i32 / 2, DISPLAY_HEIGHT as i32 / 2),
                                text_style,
                                Alignment::Center,
                            )
                            .draw(display.deref_mut())
                            .unwrap();
                        }
                    })
                    .await;
                }
                self.light_button(BACK_MIDI, MoveColor::LightGray as _);
            }
            _ => (),
        }
    }

    async fn handle_event(&mut self, e: Events) {
        let was_main = match self.topsm.state() {
            top::States::Main => true,
            _ => false,
        };

        if let Some(ns) = self.topsm.process_event(e) {
            use top::States;
            match ns {
                States::LaunchMove => {
                    self.display_centered("Launching Move").await;
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    self.exit_cmd = Some(ExitCmd::LaunchMove);
                }
                States::PowerOff => {
                    self.display_centered("Powering Down").await;

                    self.light_button(BACK_MIDI, 0);
                    self.light_button(MENU_MIDI, 0);

                    //leave some time for it do draw
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    self.send_power_cmd(PowerCommand::PowerOff);
                }
                States::PromptExit(selected) => {
                    let selected: usize = *selected;
                    self.with_display(|display| {
                        draw_menu(display, &"Exit RNBO", &EXIT_MENU, selected, None);
                    })
                    .await;
                    self.light_button(BACK_MIDI, MoveColor::LightGray as _);
                }
                States::VolumeEditor => {
                    let volume = self.volume();
                    self.light_button(BACK_MIDI, MoveColor::LightGray as _);
                    self.display_centered("Volume").await;
                    self.with_display(|mut display| {
                        display.clear(BinaryColor::Off).unwrap();
                        draw_title(&mut display, &"Volume");
                        let volume = format!("{:.2}", volume);
                        draw_centered(&mut display, volume.as_str(), TITLE_TEXT_STYLE);
                    })
                    .await;
                }
                _ => (),
            }
        }

        //println!("top state {:?}", self.topsm.state());

        match self.topsm.state() {
            top::States::Main => {
                //if we're coming out of volume into a parameter editor for instance, we want to
                //know what we've touched
                let touch = match e {
                    Events::EncTouch(e) => e < 8,
                    _ => false,
                };
                let render = if was_main || touch {
                    let ns = self.sm.process_event(e);
                    ns.is_some()
                } else {
                    //if top transitioned, we don't process an event but we do render
                    true
                };
                if render {
                    let s = self.sm.state().clone();
                    self.render_state(&s).await;
                }
            }
            _ => {
                //pass thru  pending changes like sets names changed etc even if
                let _ = match e {
                    Events::ParamUpdate(_)
                    | Events::Transport(_)
                    | Events::Tempo(_)
                    | Events::SetNamesChanged
                    | Events::SetPresetNamesChanged
                    | Events::SetCurrentChanged
                    | Events::SetPresetLoadedChanged => self.sm.process_event(e),
                    _ => None,
                };
            }
        }

        self.process_cmds().await;
    }

    fn render_param(&mut self, index: usize, location: usize) {
        let color = if let Some(param) = self.params.get(index) {
            let cap = 0.96;
            let v = param.norm_prefer_pending();

            //TODO get from metdata?
            Srgb::new(1.0, 1.0, 1.0).darken(cap - v * cap)
        } else {
            Srgb::new(0., 0., 0.)
        }
        .into_format();

        for m in led_color(location as _, &color) {
            let _ = self.midi_out_queue.send(m);
        }
    }

    fn clear_params(&mut self) {
        for index in 0..PARAM_PAGE_SIZE {
            self.clear_param(index);
        }
    }

    fn clear_param(&mut self, location: usize) {
        let num = location + 71;
        let _ = self.midi_out_queue.send(Midi::cc(
            num as u8,
            MoveColor::Black as _,
            MOVE_CTL_MIDI_CHAN,
        ));
    }

    async fn process_cmds(&mut self) {
        while let Ok(cmd) = self.cmd_queue.try_recv() {
            match cmd {
                Cmd::Power(cmd) => self.send_power_cmd(cmd),

                Cmd::OffsetParam {
                    instance,
                    index,
                    offset,
                } => {
                    if let Some(instance) = self.instance_params.get(instance) {
                        if let Some(index) = instance.get(index) {
                            if let Some(param) = self.params.get_mut(*index) {
                                let mut args = Vec::new();
                                let step = 0.01; //TODO allow for other step sizes
                                                 //operate on the normalized value.. TODO, change step
                                let v = (param.norm() + if offset > 0 { step } else { -step })
                                    .clamp(0.0, 1.0);
                                param.set_norm(v);
                                args.push(OscType::Double(v));
                                let msg = OscMessage {
                                    addr: param.addr_norm().to_string(),
                                    args,
                                };
                                self.send_osc(msg).await;
                            }
                        }
                    }
                    //self.render_param(instance, param);
                }
                Cmd::OffsetVolume(amt) => {
                    let cur = self.config.volume as isize;
                    let next = (cur + amt).clamp(0, 255);
                    if next != cur {
                        self.config.volume = next as u8;
                        self.volume
                            .store(self.config.volume, AtomicOrdering::SeqCst);
                    }
                }
                Cmd::OffsetTempo(offset) => {
                    let v = (self.bpm + (offset as f32) * self.tempo_offset_mul).clamp(0.5, 500.0); //XXX range?
                    if v != self.bpm {
                        let msg = OscMessage {
                            addr: TRANSPORT_BPM_ADDR.to_string(),
                            args: vec![OscType::Float(v)],
                        };
                        self.send_osc(msg).await;
                    }
                }
                Cmd::MulTempoOffset(mul) => {
                    self.tempo_offset_mul = if mul { 5.0 } else { 1.0 };
                }
                Cmd::ToggleTransport => {
                    let msg = OscMessage {
                        addr: TRANSPORT_ROLLING_ADDR.to_string(),
                        args: vec![OscType::Bool(!self.rolling)],
                    };
                    self.send_osc(msg).await;
                }

                Cmd::LightButton { btn, val } => self.light_button(btn, val),

                Cmd::RenderParamPage { instance, page } => {
                    let mut indexes = Vec::new();
                    if let Some(paramindexes) = self.instance_params.get(instance) {
                        let offset = page * PARAM_PAGE_SIZE;
                        indexes = paramindexes
                            .iter()
                            .skip(offset)
                            .take(PARAM_PAGE_SIZE)
                            .map(|i| *i)
                            .collect();
                    }
                    for i in 0..PARAM_PAGE_SIZE {
                        if let Some(index) = indexes.get(i) {
                            self.render_param(*index, i);
                        } else {
                            self.clear_param(i);
                        }
                    }
                }
                Cmd::RenderParam { instance, param } => {
                    //XXX do we need some sort of throttle?
                    if let Some(paramindexes) = self.instance_params.get(instance) {
                        if let Some(index) = paramindexes.get(param) {
                            self.render_param(*index, param % PARAM_PAGE_SIZE);
                        }
                    }
                }

                Cmd::LoadSet(index) => {
                    if index == 0 {
                        let msg = OscMessage {
                            addr: INST_UNLOAD_ADDR.to_string(),
                            args: vec![OscType::Int(-1)],
                        };
                        self.send_osc(msg).await;
                    } else {
                        if let Some(name) = self.set_names.get(index) {
                            let msg = OscMessage {
                                addr: SET_LOAD_ADDR.to_string(),
                                args: vec![OscType::String(name.clone())],
                            };
                            self.send_osc(msg).await;
                            //wait for `/loaded` to actually indicate load?
                        }
                    }
                }
                Cmd::LoadSetPreset(index) => {
                    if let Some(name) = self.set_preset_names.get(index) {
                        let msg = OscMessage {
                            addr: SET_PRESETS_LOAD_ADDR.to_string(),
                            args: vec![OscType::String(name.clone())],
                        };
                        self.send_osc(msg).await;
                    }
                }

                Cmd::ClearParams => self.clear_params(),
            }
        }
    }

    async fn with_display<T, F: Fn(MutexGuard<'_, MoveDisplay>) -> T>(&self, f: F) -> T {
        let g = self.display.lock().await;
        f(g)
    }

    async fn send_osc(&mut self, msg: OscMessage) {
        if let Some(ws) = self.ws_tx.as_mut() {
            let packet = OscPacket::Message(msg);
            if let Ok(msg) = rosc::encoder::encode(&packet) {
                let _ = ws.send(Message::Binary(msg)).await;
            }
        }
    }

    fn context(&self) -> &Context {
        self.sm.context()
    }
}

fn draw_title(display: &mut MoveDisplay, title: &str) {
    Text::with_alignment(
        title,
        Point::new(DISPLAY_WIDTH as i32 / 2, 11),
        TITLE_TEXT_STYLE,
        Alignment::Center,
    )
    .draw(display)
    .unwrap();
}

fn draw_centered(display: &mut MoveDisplay, text: &str, style: MonoTextStyle<BinaryColor>) {
    Text::with_alignment(
        text,
        Point::new(DISPLAY_WIDTH as i32 / 2, DISPLAY_HEIGHT as i32 / 2),
        style,
        Alignment::Center,
    )
    .draw(display)
    .unwrap();
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

impl Drop for StateController {
    fn drop(&mut self) {
        if let Ok(file) = std::fs::File::create(&self.config_path) {
            let _ = serde_json::to_writer_pretty(file, &self.config);
        }
    }
}
