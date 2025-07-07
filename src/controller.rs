use {
    crate::{config::Config, midi::Midi, param::Param, patcher::PatcherInst, view::ParamView},
    futures_util::{stream::SplitSink, SinkExt},
    palette::{Darken, Srgb},
    ratatui::{
        layout::{Alignment, Constraint, Direction, Flex, Layout, Rect},
        style::{Color, Modifier, Style},
        text::{Line, Span, Text},
        widgets::{LineGauge, Paragraph},
    },
    reqwest_websocket::{Message, WebSocket},
    rosc::{OscMessage, OscPacket, OscType},
    std::{
        cmp::PartialEq,
        collections::HashMap,
        fs::File,
        io::BufReader,
        ops::DerefMut,
        path::{Path, PathBuf},
        rc::Rc,
        sync::{
            atomic::{AtomicU8, Ordering as AtomicOrdering},
            mpsc as sync_mpsc, Arc,
        },
        time::Duration,
    },
    tokio::sync::{Mutex, MutexGuard},
    tui_widget_list::{ListBuilder, ListState, ListView},
};

const VERSION: &'static str = env!("CARGO_PKG_VERSION");

const DATFILE_DIR: &'static str = "/data/UserData/Documents/rnbo/datafiles";

const MENU_MIDI: u8 = 0x32;
const BACK_MIDI: u8 = 0x33;
const PLAY_MIDI: u8 = 0x55;

const ANIMATION_FRAME_FREEZE: usize = 4;
const ANIMATION_FRAME_DIV: usize = 10;

const MOVE_CTL_MIDI_CHAN: u8 = 15;

const PARAM_Y_OFFSET: i32 = -6;

const TRANSPORT_ROLLING_ADDR: &str = "/rnbo/jack/transport/rolling";
const TRANSPORT_BPM_ADDR: &str = "/rnbo/jack/transport/bpm";

pub const INST_UNLOAD_ADDR: &str = "/rnbo/inst/control/unload";
pub const SET_LOAD_ADDR: &str = "/rnbo/inst/control/sets/load";
pub const SET_CURRENT_ADDR: &str = "/rnbo/inst/control/sets/current/name";
pub const SET_PRESETS_LOAD_ADDR: &str = "/rnbo/inst/control/sets/presets/load";
pub const SET_PRESETS_LOADED_ADDR: &str = "/rnbo/inst/control/sets/presets/loaded";
pub const SET_VIEWS_LIST_ADDR: &str = "/rnbo/inst/control/sets/views/list";
pub const SET_VIEWS_ORDER_ADDR: &str = "/rnbo/inst/control/sets/views/order";

pub const SET_VIEW_DISPLAY: &str = "/rnboctl/view/display";
pub const SET_VIEW_PAGE_DISPLAY: &str = "/rnboctl/view/page";

const JOG_WHEEL_TOUCH: usize = 9;
const VOLUME_WHEEL_TOUCH: usize = 8;

const VOLUME_WHEEL_ENCODER: usize = 9;
const JOG_WHEEL_ENCODER: usize = 10;

const PARAM_PAGE_SIZE: usize = 8;

fn param_pages(params: usize) -> usize {
    params / PARAM_PAGE_SIZE + if params % PARAM_PAGE_SIZE == 0 { 0 } else { 1 }
}

fn all_enabled(_: usize) -> bool {
    true
}
fn default_indicator(_: usize) -> &'static char {
    ITEM_INDICATOR
}

fn animate_text<'a, T>(content: T, width: u16, frame: usize) -> String
where
    T: Into<std::borrow::Cow<'a, str>>,
{
    let line = content.into().into_owned();
    let width: usize = width.into();
    if line.len() > width {
        let movelen = line.len() - width;
        let fmovelen = movelen as f64;
        let animlen = 2 * (ANIMATION_FRAME_FREEZE + movelen);
        let index = (frame / ANIMATION_FRAME_DIV) % animlen;
        let index = if index < ANIMATION_FRAME_FREEZE + movelen {
            (index.saturating_sub(ANIMATION_FRAME_FREEZE) as f64) / fmovelen
        } else if index < ANIMATION_FRAME_FREEZE * 2 + movelen {
            1.0
        } else {
            1.0 - (index - (ANIMATION_FRAME_FREEZE * 2 + movelen)) as f64 / fmovelen
        } * fmovelen;

        let index = index as usize;

        let line = line.split_at(index).1;
        if line.len() > width {
            line.split_at(width).0
        } else {
            line
        }
        .to_string()
    } else {
        line
    }
}

fn format_title<'a, T>(content: T) -> ratatui::text::Line<'a>
where
    T: Into<std::borrow::Cow<'a, str>>,
{
    let style = Style::default()
        .fg(Color::White)
        .add_modifier(Modifier::UNDERLINED);
    ratatui::text::Line::styled(content, style).centered()
}

fn titled_layout(rect: ratatui::layout::Rect) -> Rc<[ratatui::layout::Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![
            Constraint::Length(1),
            Constraint::Length(rect.height - 1),
        ])
        .split(rect)
}

fn param_layout(rect: ratatui::layout::Rect) -> Rc<[ratatui::layout::Rect]> {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(rect)
}

struct ParamFocus {
    label: String,
    value: String,
    norm: f64,
}

fn render_param_page(
    frame: &mut ratatui::Frame,
    title: &str,
    focus: Option<ParamFocus>,
    page: usize,
    pages: usize,
) {
    let layout = param_layout(frame.area());

    let width = frame.area().width;
    let title = format_title(animate_text(title, width, frame.count()));
    frame.render_widget(title, layout[0]);

    if let Some(focus) = focus {
        let name = Line::from(animate_text(focus.label, width, frame.count()));
        frame.render_widget(name, layout[1]);

        let label = Span::raw(focus.value);
        /*
        let gauge = Gauge::default()
        .label(label)
        .gauge_style(Style::new().fg(Color::White).bg(Color::Black))
        .ratio(focus.norm)
        .use_unicode(false) //XXX when we get a better font?
        ;
        */

        let gauge = LineGauge::default()
            .label(label)
            .filled_style(Style::new().white().on_black().bold())
            .unfilled_style(Style::new().black().on_black().bold())
            .ratio(focus.norm);
        frame.render_widget(gauge, layout[2]);
    }

    if pages > 1 {
        let width: usize = (pages as f64).log10().floor() as usize + 1;
        let label = format!("{:0width$}/{}", page + 1, pages, width = width);
        let label = Span::raw(label);
        let ratio = (page as f64 + 1.0) / (pages as f64);
        let gauge = LineGauge::default()
            .label(label)
            .filled_style(Style::new().white().on_black().bold())
            .unfilled_style(Style::new().black().on_black().bold())
            .ratio(ratio);
        frame.render_widget(gauge, layout[3]);
    }
}

fn render_menu<SI: AsRef<str>, FS: Fn(usize) -> &'static char, FE: Fn(usize) -> bool>(
    frame: &mut ratatui::Frame,
    title: Option<&str>,
    items: &[SI],
    selector: FS,
    enabled: FE,
    selected: usize,
    indicated: Option<usize>,
) {
    let label_width = frame.area().width - 2;
    let frame_index = frame.count();
    let builder = ListBuilder::new(|context| {
        use crate::widget::menu::MenuItem;
        let indicated = Some(context.index) == indicated;
        let s: &str = items[context.index].as_ref();
        let mut item = if context.is_selected {
            let selector = selector(context.index);
            let s = animate_text(s, label_width, frame_index);
            MenuItem::new_selected(s, indicated, selector)
        } else {
            MenuItem::new(s, indicated)
        };

        // Style the selected element
        if context.is_selected {
            item.style = item.style.add_modifier(ratatui::style::Modifier::BOLD);
        }

        if !enabled(context.index) {
            item.style = item
                .style
                .add_modifier(ratatui::style::Modifier::CROSSED_OUT);
        }

        (item, 1)
    });

    let list = ListView::new(builder, items.len()).scroll_padding(1);
    let mut state = ListState::default();
    state.select(Some(selected));

    let mut listrect = frame.area().clone();
    if let Some(title) = title {
        let layout = titled_layout(frame.area());
        let title = format_title(title);
        frame.render_widget(title, layout[0]);
        listrect = layout[1];
    }
    frame.render_stateful_widget(list, listrect, &mut state);
}

fn center_vertical(area: Rect, height: u16) -> Rect {
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    area
}

fn center_horizontal(area: Rect, width: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    area
}
fn center(area: Rect, horizontal: Constraint, vertical: Constraint) -> Rect {
    let [area] = Layout::horizontal([horizontal])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([vertical]).flex(Flex::Center).areas(area);
    area
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum MenuIndicator {
    SubMenu(usize),
    Item(usize),
}

const SUB_MENU_INDICATOR: &'static char = &'>';
const ITEM_INDICATOR: &'static char = &'-';

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum InstSelType {
    Params,
    Datarefs,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InstSel {
    pub selected: usize,
    pub count: usize,
    pub typ: InstSelType,
}

impl InstSel {
    pub fn new(typ: InstSelType, selected: usize, count: usize) -> Self {
        Self {
            selected,
            count,
            typ,
        }
    }
    pub fn enter(typ: InstSelType, count: usize) -> Self {
        Self {
            selected: 0,
            count,
            typ,
        }
    }
    pub fn selected(&self) -> usize {
        self.selected
    }
    pub fn typ(&self) -> InstSelType {
        self.typ
    }

    pub fn can_go_next(&self) -> bool {
        self.selected + 1 < self.count
    }
    pub fn can_go_prev(&self) -> bool {
        self.selected > 0
    }

    pub fn next(&self) -> Self {
        let mut v = self.clone();
        let selected = v.selected + 1;
        if selected < v.count {
            v.selected = selected;
        }
        v
    }
    pub fn prev(&self) -> Self {
        let mut v = self.clone();
        if v.selected > 0 {
            v.selected = v.selected - 1;
        }
        v
    }

    pub fn restart(&self) -> Self {
        let mut v = self.clone();
        v.selected = 0;
        v
    }
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

    Transport(bool),
    Tempo(f32),

    SetViewSelected((usize, usize)), //index, page
    SetViewPageSelected(usize),

    VisibleParamUpdated(usize),

    InstancesChanged(usize),
    DatarefMappingChanged,
    DatarefVisibleChanged,

    SetNamesChanged,
    SetPresetNamesChanged,

    SetCurrentChanged,
    SetPresetLoadedChanged,

    SetViewListChanged,

    ChildProcessError,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParamPage {
    index: usize, //not instance index, index within out list
    page: usize,
    focused: Option<usize>,
}

impl ParamPage {
    fn offset_page(&self, offset: isize) -> usize {
        (self.page as isize + offset).max(0) as usize
    }
    fn with_offset_page(&self, offset: isize) -> Self {
        let mut p = self.clone();
        p.page = (p.page as isize + offset).max(0) as usize;
        p
    }
    fn with_focus(&self, index: usize) -> Self {
        let mut p = self.clone();
        p.focused = Some(index);
        p
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DataSel {
    instance: usize, //not instance index, index within out list
    selected: usize,
    count: usize,
}

impl DataSel {
    pub fn new(instance: usize, dataref_count: usize) -> Self {
        Self {
            instance,
            selected: 0,
            count: dataref_count,
        }
    }

    pub fn instance(&self) -> usize {
        self.instance
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn can_go_prev(&self) -> bool {
        self.selected > 0
    }

    pub fn can_go_next(&self) -> bool {
        self.selected + 1 < self.count
    }

    pub fn prev(&self) -> DataSel {
        let mut v = self.clone();
        if self.can_go_prev() {
            v.selected = v.selected - 1;
        }
        v
    }

    pub fn next(&self) -> DataSel {
        let mut v = self.clone();
        if self.can_go_next() {
            v.selected = v.selected + 1;
        }
        v
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DataLoad {
    dataref: DataSel,
    selected: usize,
    filecount: usize,
}

impl DataLoad {
    pub fn new(dataref: DataSel, filecount: usize) -> Self {
        Self {
            dataref,
            selected: 0,
            filecount,
        }
    }

    pub fn dataload_cmd(&self) -> Cmd {
        Cmd::LoadDataref((
            self.dataref.instance(),
            self.dataref.selected(),
            self.selected,
        ))
    }

    pub fn dataref(&self) -> DataSel {
        self.dataref.clone()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn can_go_prev(&self) -> bool {
        self.selected > 0
    }

    pub fn can_go_next(&self) -> bool {
        self.selected + 1 < self.filecount
    }

    pub fn prev(&self) -> Self {
        let mut v = self.clone();
        if self.can_go_prev() {
            v.selected = v.selected - 1;
        }
        v
    }

    pub fn next(&self) -> Self {
        let mut v = self.clone();
        if self.can_go_next() {
            v.selected = v.selected + 1;
        }
        v
    }
}

const MENU_ITEMS: [&'static str; 6] = [
    "Device Params",
    "Device Data",
    "Graphs",
    "Graph Presets",
    "Tempo",
    "About",
];
const EXIT_MENU: [&'static str; 2] = ["Power Down", "Launch Move"];

const PATCHER_PARAMS_INDEX: usize = 0;
const PATCHER_DATA_INDEX: usize = 1;
const SETS_INDEX: usize = 2;
const SET_PRESETS_INDEX: usize = 3;
const TEMPO_INDEX: usize = 4;
const ABOUT_INDEX: usize = 5;

#[derive(Clone, Debug, PartialEq)]
enum Cmd {
    Power(PowerCommand),

    OffsetParam {
        instance: usize,
        index: usize,
        offset: isize,
    },
    OffsetViewParam {
        view: usize,
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

    RenderVisibleParams,

    UpdateDataFileList,
    //local index, dataref index, file index
    LoadDataref((usize, usize, usize)),

    LoadSet(usize),
    LoadSetPreset(usize),

    ReportViewParamPage(usize, usize),
}

pub mod top {
    use super::{
        Button, Cmd, Context, Events, PowerCommand, EXIT_MENU, JOG_WHEEL_ENCODER, JOG_WHEEL_TOUCH,
        VOLUME_WHEEL_ENCODER, VOLUME_WHEEL_TOUCH,
    };

    const POWER_DOWN_INDEX: usize = 0;
    const LAUNCH_MOVE_INDEX: usize = 1;

    #[derive(PartialEq, Eq, Clone, Copy, Debug)]
    pub(crate) enum LastView {
        Main,
        ParamViews,
    }

    smlang::statemachine! {
        states_attr: #[derive(Clone, Debug)],
        transitions: {
            *Init + BtnDown(Button::Menu) = Main,
            Init + BtnDown(Button::JogWheel) = Main,
            Init + BtnDown(Button::Back) = Main,
            Init + EncTouch(JOG_WHEEL_TOUCH) = Main,

            Init + EncTouch(VOLUME_WHEEL_TOUCH) = VolumeEditor(LastView::Main),
            Init + EncRight(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(1)); = VolumeEditor(LastView::Main),
            Init + EncLeft(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(-1)); = VolumeEditor(LastView::Main),

            Main + EncTouch(VOLUME_WHEEL_TOUCH) = VolumeEditor(LastView::Main),
            Main + EncRight(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(1)); = VolumeEditor(LastView::Main),
            Main + EncLeft(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(-1)); = VolumeEditor(LastView::Main),

            //toggle
            Main + BtnDown(Button::Menu) / ctx.emit(Cmd::RenderVisibleParams); = ParamViews,
            ParamViews + BtnDown(Button::Menu)/ ctx.emit(Cmd::RenderVisibleParams);  = Main,

            ParamViews + EncTouch(VOLUME_WHEEL_TOUCH)  = VolumeEditor(LastView::ParamViews),
            ParamViews + EncRight(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(1)); = VolumeEditor(LastView::ParamViews),
            ParamViews + EncLeft(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(-1)); = VolumeEditor(LastView::ParamViews),

            VolumeEditor(LastView) + BtnDown(Button::Back) [*state == LastView::Main] / ctx.emit(Cmd::RenderVisibleParams); = Main,
            VolumeEditor(LastView) + BtnDown(Button::Menu) [*state == LastView::ParamViews] / ctx.emit(Cmd::RenderVisibleParams); = Main,
            VolumeEditor(LastView) + BtnDown(Button::Back) [*state == LastView::ParamViews] / ctx.emit(Cmd::RenderVisibleParams); = ParamViews,
            VolumeEditor(LastView) + BtnDown(Button::Menu) [*state == LastView::Main] / ctx.emit(Cmd::RenderVisibleParams); = ParamViews,
            VolumeEditor(LastView) + EncRight(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(1)); = VolumeEditor(*state),
            VolumeEditor(LastView) + EncLeft(VOLUME_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetVolume(-1)); = VolumeEditor(*state),
            VolumeEditor(LastView) + EncTouch(_) [*event != VOLUME_WHEEL_TOUCH && *state == LastView::Main] / ctx.emit(Cmd::RenderVisibleParams); = Main,
            VolumeEditor(LastView) + EncTouch(_) [*event != VOLUME_WHEEL_TOUCH && *state == LastView::ParamViews] / ctx.emit(Cmd::RenderVisibleParams); = ParamViews,

            PromptExit(usize) + BtnDown(Button::JogWheel) [*state == POWER_DOWN_INDEX] = PowerOff,
            PromptExit(usize) + BtnDown(Button::JogWheel) [*state == LAUNCH_MOVE_INDEX] = LaunchMove,
            PromptExit(usize) + EncRight(JOG_WHEEL_ENCODER) [*state + 1 < EXIT_MENU.len()] = PromptExit(*state + 1),
            PromptExit(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = PromptExit(*state - 1),
            PromptExit(usize) + BtnDown(Button::Back) [ctx.can_exit_powermenu()] / ctx.emit(Cmd::RenderVisibleParams); = Main,
            PromptExit(usize) + BtnDown(Button::Menu) [ctx.can_exit_powermenu()] / ctx.emit(Cmd::RenderVisibleParams); = Main,

            _ + BtnDown(Button::PowerShort) / ctx.emit(Cmd::Power(PowerCommand::ClearShortPress)); = PromptExit(POWER_DOWN_INDEX),
            _ + BtnDown(Button::PowerLong) / ctx.emit(Cmd::Power(PowerCommand::ClearLongPress)); = PowerOff,

            _ + BtnDown(Button::Play) / ctx.emit(Cmd::ToggleTransport);,

            Main + SetViewSelected(_) = ParamViews,
            Main + SetViewPageSelected(_) = ParamViews,
            VolumeEditor(LastView) + SetViewSelected(_) = ParamViews,
            VolumeEditor(LastView) + SetViewPageSelected(_) = ParamViews,

            _ + ChildProcessError = DisplayChildProcessError,
            DisplayChildProcessError + BtnDown(Button::PowerShort) / ctx.emit(Cmd::Power(PowerCommand::ClearShortPress)); = PromptExit(POWER_DOWN_INDEX),
        }
    }
}

pub mod view {
    use super::{Button, Cmd, Context, Events, ParamPage, JOG_WHEEL_ENCODER, PARAM_PAGE_SIZE};
    smlang::statemachine! {
        states_attr: #[derive(Clone, Debug)],
        transitions: {
            *ParamViewMenu(usize) + EncRight(JOG_WHEEL_ENCODER) [*state + 1 < ctx.param_view_count()] = ParamViewMenu(*state + 1),
            ParamViewMenu(usize) + EncLeft(JOG_WHEEL_ENCODER) [*state > 0] = ParamViewMenu(*state - 1),
            ParamViewMenu(usize) + BtnDown(Button::JogWheel) /
                { ctx.emit(Cmd::ReportViewParamPage(*state, 0)); ctx.emit(Cmd::RenderVisibleParams); }=
                ViewParams(ParamPage { index: *state, page: 0, focused: None }),

            ViewParams(ParamPage) + BtnDown(Button::Back) = ParamViewMenu(state.index),
            ViewParams(ParamPage) + EncRight(JOG_WHEEL_ENCODER) [state.page + 1 < ctx.view_param_pages(state.index)] /
                { ctx.emit(Cmd::ReportViewParamPage(state.index, state.offset_page(1))); ctx.emit(Cmd::RenderVisibleParams); }=
                ViewParams(state.with_offset_page(1)),
            ViewParams(ParamPage) + EncLeft(JOG_WHEEL_ENCODER) [state.page > 0] /
                { ctx.emit(Cmd::ReportViewParamPage(state.index, state.offset_page(-1))); ctx.emit(Cmd::RenderVisibleParams); }=
                ViewParams(state.with_offset_page(-1)),
            ViewParams(ParamPage) + EncTouch(_) [*event < 8] = ViewParams(state.with_focus(*event)),
            ViewParams(ParamPage) + EncLeft(_) [*event < 8] / ctx.emit(Cmd::OffsetViewParam { view: state.index, index: state.page * PARAM_PAGE_SIZE + *event, offset: -1}); = ViewParams(state.with_focus(*event)),
            ViewParams(ParamPage) + EncRight(_) [*event < 8] / ctx.emit(Cmd::OffsetViewParam { view: state.index, index: state.page * PARAM_PAGE_SIZE + *event, offset: 1}); = ViewParams(state.with_focus(*event)),
            ViewParams(ParamPage) + VisibleParamUpdated(_) [Some(*event) == state.focused] = ViewParams(state.clone()), //redraw

            ParamViewMenu(usize) + SetViewSelected(_) [event.0 < ctx.param_view_count() && event.1 < ctx.view_param_pages(event.0)] /
                { ctx.emit(Cmd::ReportViewParamPage(event.0, event.1)); ctx.emit(Cmd::RenderVisibleParams); }=
                ViewParams(ParamPage { index: event.0, page: event.1, focused: None }),
            ViewParams(ParamPage) + SetViewSelected(_)
                [(state.index != event.0 || state.page != event.1) && event.0 < ctx.param_view_count() && event.1 < ctx.view_param_pages(event.0)] /
                { ctx.emit(Cmd::ReportViewParamPage(event.0, event.1)); ctx.emit(Cmd::RenderVisibleParams); }=
                ViewParams(ParamPage { index: event.0, page: event.1, focused: state.focused }),

            ViewParams(ParamPage) + SetViewPageSelected(_) [state.page != *event] /
                { ctx.emit(Cmd::ReportViewParamPage(state.index, (*event).min(ctx.view_param_pages(state.index) - 1))); ctx.emit(Cmd::RenderVisibleParams);} =
                ViewParams(ParamPage { index: state.index, page: (*event).min(ctx.view_param_pages(state.index) - 1), focused: state.focused }),

            _ + SetViewListChanged = ParamViewMenu(0),
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
        Menu(usize) + BtnDown(Button::JogWheel) [*state == PATCHER_PARAMS_INDEX && ctx.instances_count(InstSelType::Params) > 1] = PatcherInstances(InstSel::enter(InstSelType::Params, ctx.instances_count(InstSelType::Params))),
        Menu(usize) + BtnDown(Button::JogWheel) [*state == PATCHER_PARAMS_INDEX && ctx.instances_count(InstSelType::Params) == 1] / ctx.emit(Cmd::RenderVisibleParams);
            = PatcherParams(ParamPage { index: 0, page: 0, focused: None }),
        Menu(usize) + BtnDown(Button::JogWheel) [*state == PATCHER_DATA_INDEX && ctx.instances_count(InstSelType::Datarefs) > 1] = PatcherInstances(InstSel::enter(InstSelType::Datarefs, ctx.instances_count(InstSelType::Datarefs))),
        Menu(usize) + BtnDown(Button::JogWheel) [*state == PATCHER_DATA_INDEX && ctx.instances_count(InstSelType::Datarefs) == 1] / ctx.emit(Cmd::UpdateDataFileList); = PatcherDatarefs(DataSel::new(0, ctx.dataref_count(0))),

        Menu(usize) + BtnDown(Button::JogWheel) [*state == TEMPO_INDEX] = TempoEditor,
        Menu(usize) + BtnDown(Button::JogWheel) [*state == ABOUT_INDEX] = About,

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

        PatcherInstances(InstSel) + BtnDown(Button::Back) [state.typ() == InstSelType::Params] = Menu(PATCHER_PARAMS_INDEX),
        PatcherInstances(InstSel) + BtnDown(Button::Back) [state.typ() == InstSelType::Datarefs] = Menu(PATCHER_DATA_INDEX),
        PatcherInstances(InstSel) + EncRight(JOG_WHEEL_ENCODER) [state.can_go_next()] = PatcherInstances(state.next()),
        PatcherInstances(InstSel) + EncLeft(JOG_WHEEL_ENCODER) [state.can_go_prev()] = PatcherInstances(state.prev()),
        PatcherInstances(InstSel) + BtnDown(Button::JogWheel) [state.typ() == InstSelType::Params] / ctx.emit(Cmd::RenderVisibleParams);
            = PatcherParams(ParamPage { index: state.selected(), page: 0, focused: None }),
        PatcherInstances(InstSel) + BtnDown(Button::JogWheel) [state.typ() == InstSelType::Datarefs] / ctx.emit(Cmd::UpdateDataFileList); = PatcherDatarefs(DataSel::new(state.selected(), ctx.dataref_count(state.selected()))),


        PatcherInstances(InstSel) + InstancesChanged(_) [ctx.instances_count(state.typ()) == 0] = Menu(if state.typ == InstSelType::Params { PATCHER_PARAMS_INDEX } else { PATCHER_DATA_INDEX }),
        PatcherInstances(InstSel) + InstancesChanged(_) [ctx.instances_count(state.typ()) > 0] = PatcherInstances(state.restart()),

        //skip patcher instances menu if there is only 1 instance
        PatcherParams(ParamPage) + BtnDown(Button::Back) [ctx.instances_count(InstSelType::Params) > 1] = PatcherInstances(InstSel::new(InstSelType::Params, state.index, ctx.instances_count(InstSelType::Params))),
        PatcherParams(ParamPage) + BtnDown(Button::Back) [ctx.instances_count(InstSelType::Params) == 1] = Menu(PATCHER_PARAMS_INDEX),
        PatcherDatarefs(DataSel) + BtnDown(Button::Back) [ctx.instances_count(InstSelType::Datarefs) > 1] = PatcherInstances(InstSel::new(InstSelType::Datarefs, state.instance(), ctx.instances_count(InstSelType::Datarefs))),
        PatcherDatarefs(DataSel) + BtnDown(Button::Back) [ctx.instances_count(InstSelType::Datarefs) == 1] = Menu(PATCHER_DATA_INDEX),

        PatcherParams(ParamPage) + EncRight(JOG_WHEEL_ENCODER) [ctx.instance_param_pages(state.index) > state.page + 1] / ctx.emit(Cmd::RenderVisibleParams);
            = PatcherParams(ParamPage { index: state.index, page: state.page + 1, focused: state.focused }),
        PatcherParams(ParamPage) + EncLeft(JOG_WHEEL_ENCODER) [state.page > 0] / ctx.emit(Cmd::RenderVisibleParams);
            = PatcherParams(ParamPage { index: state.index, page: state.page - 1, focused: state.focused }),
        PatcherParams(ParamPage) + EncTouch(_) [*event < 8] = PatcherParams(state.with_focus(*event)),
        PatcherParams(ParamPage) + EncLeft(_) [*event < 8] / ctx.emit(Cmd::OffsetParam { instance: state.index, index: state.page * PARAM_PAGE_SIZE + *event, offset: -1}); = PatcherParams(state.with_focus(*event)),
        PatcherParams(ParamPage) + EncRight(_) [*event < 8] / ctx.emit(Cmd::OffsetParam { instance: state.index, index: state.page * PARAM_PAGE_SIZE + *event, offset: 1}); = PatcherParams(state.with_focus(*event)),

        PatcherParams(ParamPage) + InstancesChanged(_) [ctx.instances_count(InstSelType::Params) == 0] = Menu(PATCHER_PARAMS_INDEX),
        PatcherParams(ParamPage) + InstancesChanged(_) [ctx.instances_count(InstSelType::Params) > 0] = PatcherInstances(InstSel::enter(InstSelType::Params, ctx.instances_count(InstSelType::Params))),
        PatcherDatarefs(DataSel) + InstancesChanged(_) [ctx.dataref_count(0) == 0] = Menu(PATCHER_DATA_INDEX),
        PatcherDatarefs(DataSel) + InstancesChanged(_) [ctx.dataref_count(0) > 0] = PatcherInstances(InstSel::enter(InstSelType::Datarefs, ctx.instances_count(InstSelType::Datarefs))),
        PatcherDatarefLoad(DataLoad) + InstancesChanged(_) [ctx.dataref_count(0) == 0] = Menu(PATCHER_DATA_INDEX),
        PatcherDatarefLoad(DataLoad) + InstancesChanged(_) [ctx.dataref_count(0) > 0] = PatcherInstances(InstSel::enter(InstSelType::Datarefs, ctx.instances_count(InstSelType::Datarefs))),

        PatcherDatarefs(DataSel) + EncRight(JOG_WHEEL_ENCODER) [state.can_go_next()] = PatcherDatarefs(state.next()),
        PatcherDatarefs(DataSel) + EncLeft(JOG_WHEEL_ENCODER) [state.can_go_prev()] = PatcherDatarefs(state.prev()),

        PatcherDatarefs(DataSel) + BtnDown(Button::JogWheel) = PatcherDatarefLoad(DataLoad::new(state.clone(), ctx.datafile_count())),

        PatcherDatarefLoad(DataLoad) + BtnDown(Button::JogWheel) / ctx.emit(state.dataload_cmd()); = PatcherDatarefLoad(state.clone()),
        PatcherDatarefLoad(DataLoad) + BtnDown(Button::Back) = PatcherDatarefs(state.dataref()),

        PatcherDatarefLoad(DataLoad) + EncRight(JOG_WHEEL_ENCODER) [state.can_go_next()] = PatcherDatarefLoad(state.next()),
        PatcherDatarefLoad(DataLoad) + EncLeft(JOG_WHEEL_ENCODER) [state.can_go_prev()] = PatcherDatarefLoad(state.prev()),
        PatcherDatarefLoad(DataLoad) + DatarefMappingChanged = PatcherDatarefLoad(state.clone()), //redraw, TODO filter to only redraw if it is a dataref we care about?

         //TODO can we be less drastic?
        PatcherDatarefs(DataSel) + DatarefVisibleChanged = Menu(PATCHER_DATA_INDEX),
        PatcherDatarefLoad(DataLoad) + DatarefVisibleChanged = Menu(PATCHER_DATA_INDEX),
        PatcherInstances(InstSel) + DatarefVisibleChanged [state.typ() == InstSelType::Datarefs] = Menu(PATCHER_DATA_INDEX),

        PatcherInstances(InstSel) + SetCurrentChanged = Menu(PATCHER_PARAMS_INDEX),
        PatcherParams(ParamPage) + SetCurrentChanged  = Menu(PATCHER_PARAMS_INDEX),
        PatcherDatarefs(DataSel) + SetCurrentChanged  = Menu(PATCHER_DATA_INDEX),
        PatcherDatarefLoad(DataLoad) + SetCurrentChanged  = Menu(PATCHER_DATA_INDEX),
        PatcherParams(ParamPage) + VisibleParamUpdated(_) [Some(*event) == state.focused] = PatcherParams(state.clone()), //redraw

        TempoEditor + BtnDown(Button::Back) = Menu(TEMPO_INDEX),
        TempoEditor + EncRight(JOG_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetTempo(1)); = TempoEditor,
        TempoEditor + EncLeft(JOG_WHEEL_ENCODER) / ctx.emit(Cmd::OffsetTempo(-1)); = TempoEditor,
        TempoEditor + BtnDown(Button::JogWheel) / ctx.emit(Cmd::MulTempoOffset(true)); = TempoEditor,
        TempoEditor + BtnUp(Button::JogWheel) / ctx.emit(Cmd::MulTempoOffset(false));  = TempoEditor,
        TempoEditor + Tempo(_) = TempoEditor,

        About + BtnDown(Button::Back) = Menu(ABOUT_INDEX),
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct Caps {
    pub memlock: bool,
    pub rtprio: bool,
}

impl Caps {
    pub fn all(&self) -> bool {
        self.memlock && self.rtprio
    }
}

pub struct StateController {
    line_token: u32, //used for "once" actions (lighting leds from render methods)
    tracked_buttons: HashMap<u8, MoveColor>,

    set_current_name: Option<String>,
    set_preset_loaded_name: Option<String>,

    set_current_index: Option<usize>,
    set_preset_loaded_index: Option<usize>,

    sysex: Vec<u8>,

    exit: bool,

    sm: StateMachine,
    viewsm: view::StateMachine,
    topsm: top::StateMachine,

    has_all_capabilities: bool,

    cmd_queue: sync_mpsc::Receiver<Cmd>,

    ws_tx: Option<SplitSink<WebSocket, Message>>,
    midi_out_queue: sync_mpsc::SyncSender<Midi>,
    volume: Arc<AtomicU8>,

    config: Config,
    config_path: PathBuf,

    rolling: bool,
    bpm: f32,
    tempo_offset_mul: f32,

    instances: Vec<PatcherInst>,

    params: Vec<Param>,

    instance_params: Vec<Vec<usize>>,

    visible_params: Vec<usize>,

    //(sparce instance index, param_id) -> (local instance_index, param index)
    instance_param_map: HashMap<(usize, String), (usize, usize)>,

    param_lookup: HashMap<String, usize>, //OSC addr -> index into self.params
    param_norm_lookup: HashMap<String, usize>, //OSC addr -> index into self.params
    dataref_lookup: HashMap<String, (usize, String)>, //OSC addr -> (index into self.instances, datarefname)
    dataref_meta_lookup: HashMap<String, (usize, String)>, //OSC addr -> (index into self.instances, datarefname)

    param_views: Vec<ParamView>,
    param_view_order: Vec<usize>,
    param_view_names: Vec<String>,
    param_view_params: Vec<Vec<usize>>,
    param_view_param_lookup: HashMap<String, usize>, //OSC addr -> index into self.param_views

    set_names: Vec<String>,
    set_preset_names: Vec<String>,

    patchers_params_instance_names: Vec<String>, //only those that have params
    patchers_datarefs_instance_names: Vec<String>, //only those that have datarefs

    patchers_params_instance_indexes: Vec<usize>, //only those that have params, index into self.params
    patchers_datarefs_instance_indexes: Vec<usize>, //only those that have datarefs, index into self.instances

    child_process_error: Option<(String, std::io::Result<std::process::ExitStatus>)>,

    runner_rnbo_version: Option<String>,

    package_version: Option<String>,

    datafile_list: Vec<String>,
    datafile_menu: Vec<String>,
}

#[derive(Clone, Debug)]
struct CommonContext {
    pub(crate) sets_count: usize,
    pub(crate) set_presets_count: usize,
    pub(crate) instances_count: HashMap<InstSelType, usize>,

    //sorted list of instances that have params, and the count of pages
    pub(crate) instance_param_pages: Vec<usize>,
    pub(crate) param_view_pages: Vec<usize>,

    //sorted list of instances that have datarefs, and the count of datarefs
    pub(crate) dataref_count: Vec<usize>,
    pub(crate) datafile_count: usize,

    pub(crate) can_exit_powermenu: bool,
}

impl Default for CommonContext {
    fn default() -> Self {
        Self {
            sets_count: 0,
            set_presets_count: 0,
            instances_count: Default::default(),

            instance_param_pages: Vec::new(),

            param_view_pages: Vec::new(),

            dataref_count: Vec::new(),
            datafile_count: 0,

            can_exit_powermenu: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Context {
    cmd_queue: sync_mpsc::Sender<Cmd>,
    common: CommonContext,
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

    fn instances_count(&self, typ: InstSelType) -> usize {
        *self.common.instances_count.get(&typ).unwrap_or(&0)
    }

    fn instance_param_pages(&self, instance: usize) -> usize {
        *self.common.instance_param_pages.get(instance).unwrap_or(&0)
    }

    fn view_param_pages(&self, view: usize) -> usize {
        *self.common.param_view_pages.get(view).unwrap_or(&0)
    }

    fn param_view_count(&self) -> usize {
        self.common.param_view_pages.len()
    }

    fn dataref_count(&self, index: usize) -> usize {
        *self.common.dataref_count.get(index).unwrap_or(&0)
    }

    fn datafile_count(&self) -> usize {
        self.common.datafile_count
    }

    fn can_exit_powermenu(&self) -> bool {
        self.common.can_exit_powermenu
    }
}

impl StateController {
    pub fn new(
        midi_out_queue: sync_mpsc::SyncSender<Midi>,
        volume: Arc<AtomicU8>,
        package_version: Option<String>,
        config_path: PathBuf,
        has_all_capabilities: bool,
    ) -> Self {
        let (tx, rx) = sync_mpsc::channel();

        let context = Context::new(tx.clone());

        let sm = StateMachine::new_with_state(context.clone(), States::Menu(0));
        let viewsm =
            view::StateMachine::new_with_state(context.clone(), view::States::ParamViewMenu(0));
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

        let tracked_buttons =
            HashMap::from([(MENU_MIDI, MoveColor::Black), (BACK_MIDI, MoveColor::Black)]);

        let mut s = Self {
            line_token: 0,
            tracked_buttons,

            sysex: Vec::new(),

            sm,
            viewsm,
            topsm,

            has_all_capabilities,

            midi_out_queue,
            volume,

            config,
            config_path,

            rolling: false,
            bpm: 100.0,
            tempo_offset_mul: 1.0,

            exit: false,

            set_current_name: None,
            set_preset_loaded_name: None,

            set_current_index: None,
            set_preset_loaded_index: None,

            cmd_queue: rx,

            ws_tx: None,

            instances: Vec::new(),

            params: Vec::new(),

            instance_params: Vec::new(),
            visible_params: Vec::new(),

            instance_param_map: HashMap::new(),

            param_lookup: HashMap::new(),
            param_norm_lookup: HashMap::new(),
            dataref_lookup: HashMap::new(),
            dataref_meta_lookup: HashMap::new(),

            param_views: Vec::new(),
            param_view_order: Vec::new(),
            param_view_names: Vec::new(),
            param_view_params: Vec::new(),
            param_view_param_lookup: HashMap::new(),

            set_names: Vec::new(),
            set_preset_names: Vec::new(),

            patchers_params_instance_names: Vec::new(),
            patchers_datarefs_instance_names: Vec::new(),

            patchers_params_instance_indexes: Vec::new(),
            patchers_datarefs_instance_indexes: Vec::new(),

            child_process_error: None,

            runner_rnbo_version: None,
            package_version,

            datafile_list: Vec::new(),
            datafile_menu: Vec::new(),
        };

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

    pub async fn set_instances(&mut self, mut instances: HashMap<usize, PatcherInst>) {
        let mut indexes: Vec<usize> = instances.keys().map(|k| *k).collect();
        indexes.sort();

        //XXX what about visible params?

        self.patchers_params_instance_names.clear();
        self.patchers_datarefs_instance_names.clear();
        self.patchers_params_instance_indexes.clear();
        self.patchers_datarefs_instance_indexes.clear();

        self.params.clear();
        self.instance_params.clear();
        self.instance_param_map.clear();
        self.param_lookup.clear();
        self.param_norm_lookup.clear();
        self.dataref_meta_lookup.clear();
        self.instances.clear();

        let mut common = self.sm.context().common();
        common.instance_param_pages.clear();
        common.dataref_count.clear();

        for key in indexes.iter() {
            let inst = instances.remove(key).unwrap();
            let name = format!("{}: {}", inst.index(), inst.name());

            if inst.params().len() > 0 {
                let mut instindexes = Vec::new();
                let local_instance_index = self.patchers_params_instance_names.len();
                self.patchers_params_instance_names.push(name.clone());
                self.patchers_params_instance_indexes
                    .push(local_instance_index);

                common
                    .instance_param_pages
                    .push(param_pages(inst.params().len()));

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
                        (p.instance_index(), p.name().to_string()),
                        (local_instance_index, local_param_index),
                    );
                }
                self.instance_params.push(instindexes);
            }

            {
                let local_instance_index = self.instances.len();
                let visible = inst.visible_datarefs();
                if visible.len() > 0 {
                    self.patchers_datarefs_instance_names.push(name.clone());
                    self.patchers_datarefs_instance_indexes
                        .push(local_instance_index);
                    common.dataref_count.push(visible.len());
                }
                for d in inst.dataref_mappings().keys() {
                    let addr = format!("/rnbo/inst/{}/data_refs/{}", inst.index(), d.clone());
                    self.dataref_lookup
                        .insert(addr.clone(), (local_instance_index, d.clone()));
                    let addr = format!("{}/meta", addr);
                    self.dataref_meta_lookup
                        .insert(addr, (local_instance_index, d.clone()));
                }
            }

            self.instances.push(inst);
        }

        common.instances_count.insert(
            InstSelType::Params,
            self.patchers_params_instance_names.len(),
        );
        common.instances_count.insert(
            InstSelType::Datarefs,
            self.patchers_datarefs_instance_names.len(),
        );
        self.clear_param_views(); //to be updated later
        self.update_common(common);

        self.handle_event(Events::InstancesChanged(indexes.len()))
            .await;
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

    pub async fn set_set_names(&mut self, names: Vec<String>) {
        self.set_names = names;
        self.set_names.sort();
        self.set_names.insert(0, "<empty>".to_string());

        //TODO check set_current_name

        let mut common = self.sm.context().common();
        common.sets_count = self.set_names.len();
        self.update_common(common);

        self.handle_event(Events::SetNamesChanged).await;
    }

    pub async fn set_set_preset_names(&mut self, names: Vec<String>) {
        let mut common = self.sm.context().common();
        common.set_presets_count = names.len();
        self.update_common(common);

        self.set_preset_names = names;

        self.handle_event(Events::SetPresetNamesChanged).await;
    }

    async fn update_views(&mut self, common: &mut CommonContext) {
        //TODO look for changes and only add/remove update those instead of clearing everything

        //if there are no views, add a default that has all the params in it
        if self.param_views.len() == 0 && self.params.len() > 0 {
            let params = self
                .params
                .iter()
                .map(|p| (p.instance_index(), p.name().to_string()))
                .collect();
            self.param_views
                .push(ParamView::new("Default".to_string(), params, 0));
        }

        self.param_view_names.clear();
        self.param_view_params.clear();

        common.param_view_pages.clear();

        for v in self.param_views.iter() {
            //find the param indexes indicated by the sparse (instance, param) pair
            let mut params = Vec::new();
            for sparce in v.params().iter() {
                if let Some((instance, param)) = self.instance_param_map.get(&sparce) {
                    if let Some(instance) = self.instance_params.get(*instance) {
                        if let Some(index) = instance.get(*param) {
                            params.push(*index);
                        } else {
                            eprintln!("couldn't get param at local index {}", *param);
                        }
                    } else {
                        eprintln!("couldn't get instance at local index {}", *instance);
                    }
                } else {
                    eprintln!("couldn't find instance at index {:?}", sparce);
                }
            }
            if params.len() > 0 {
                self.param_view_names.push(v.name().to_string());
                common.param_view_pages.push(param_pages(params.len()));
                self.param_view_params.push(params);
            }
        }
        //TODO check that current view is valid?
    }

    fn sort_param_views(&mut self) {
        let mut sorted = Vec::new();

        for i in self.param_view_order.iter() {
            if let Some(v) = self.param_views.iter().position(|v| v.index() == *i) {
                sorted.push(self.param_views.swap_remove(v)); //swap remove is more efficient
            } else {
                //ERROR
            }
        }
        //if there are any left over, push them all the the back
        sorted.append(&mut self.param_views);

        std::mem::swap(&mut sorted, &mut self.param_views);
    }

    pub fn clear_param_views(&mut self) {
        self.param_views.clear();
        self.param_view_param_lookup.clear();
    }

    pub async fn set_param_views(&mut self, mut views: Vec<ParamView>) {
        self.param_views = views;
        self.sort_param_views();

        //compute lookup
        self.param_view_param_lookup.clear();
        for (index, view) in self.param_views.iter().enumerate() {
            let addr = format!("{}/{}/params", SET_VIEWS_LIST_ADDR, view.index());
            self.param_view_param_lookup.insert(addr, index);
        }

        let mut common = self.sm.context().common();
        self.update_views(&mut common).await;
        self.update_common(common);
        self.handle_event(Events::SetViewListChanged).await;
    }

    pub async fn display_child_process_error(
        &mut self,
        name: &str,
        status: std::io::Result<std::process::ExitStatus>,
    ) {
        let mut common = self.sm.context().common();
        common.can_exit_powermenu = false;
        self.update_common(common);

        self.child_process_error = Some((name.to_string(), status));
        self.handle_event(Events::ChildProcessError).await;
    }

    pub fn set_runner_version(&mut self, runner_rnbo_version: &str) {
        self.runner_rnbo_version = Some(runner_rnbo_version.to_string());
    }

    pub async fn handle_osc(&mut self, msg: &OscMessage) {
        //update param view
        if let Some(index) = self.param_view_param_lookup.get(&msg.addr) {
            let updated = if let Some(view) = self.param_views.get_mut(*index) {
                let params: Result<Vec<(usize, String)>, ()> = msg
                    .args
                    .iter()
                    .map(|a| {
                        if let OscType::String(v) = a {
                            ParamView::parse_param_s(v)
                        } else {
                            Err(())
                        }
                    })
                    .collect();
                if let Ok(params) = params {
                    view.set_params(params);
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if updated {
                let mut common = self.sm.context().common();
                self.update_views(&mut common).await;
                self.update_common(common);
                //TODO transmit more fine tuned changes
                self.handle_event(Events::SetViewListChanged).await;
            }
        } else {
            //println!("got osc {}", msg.addr);
            //let mut update = None;
            match msg.addr.as_str() {
                TRANSPORT_ROLLING_ADDR => {
                    if msg.args.len() == 1 {
                        if let OscType::Bool(rolling) = msg.args[0] {
                            if self.rolling != rolling {
                                self.rolling = rolling;
                                let _ = self.midi_out_queue.send(Midi::cc(
                                    PLAY_MIDI,
                                    if rolling {
                                        MoveColor::Green
                                    } else {
                                        MoveColor::LightGray
                                    } as _,
                                    MOVE_CTL_MIDI_CHAN,
                                ));
                                self.handle_event(Events::Transport(rolling)).await;
                            }
                        }
                    }
                }
                TRANSPORT_BPM_ADDR => {
                    if msg.args.len() == 1 {
                        if let Some(bpm) = match &msg.args[0] {
                            OscType::Double(v) => Some(*v as f32),
                            OscType::Float(v) => Some(*v),
                            _ => None,
                        } {
                            self.bpm = bpm;
                            self.handle_event(Events::Tempo(bpm)).await;
                        }
                    }
                }
                SET_CURRENT_ADDR => {
                    if msg.args.len() == 1 {
                        let name = match &msg.args[0] {
                            OscType::String(name) => Some(name.clone()),
                            _ => None,
                        };
                        self.set_set_current_name(name).await;
                    }
                }
                SET_PRESETS_LOADED_ADDR => {
                    if msg.args.len() == 1 {
                        self.set_preset_loaded_name = match &msg.args[0] {
                            OscType::String(name) => Some(name.clone()),
                            _ => None,
                        };
                        self.set_preset_loaded_index =
                            if let Some(name) = &self.set_preset_loaded_name {
                                self.set_preset_names.iter().position(|r| r == name)
                            } else {
                                None
                            };
                        self.handle_event(Events::SetPresetLoadedChanged).await;
                    }
                }
                SET_VIEWS_ORDER_ADDR => {
                    self.param_view_order.clear();
                    for arg in msg.args.iter() {
                        match arg {
                            OscType::Int(i) if *i >= 0 => {
                                self.param_view_order.push(*i as usize);
                            }
                            _ => (),
                        }
                    }
                    self.sort_param_views();
                    let mut common = self.sm.context().common();
                    self.update_views(&mut common).await;
                    self.handle_event(Events::SetViewListChanged).await;
                }
                SET_VIEW_DISPLAY => {
                    if msg.args.len() > 0 {
                        if let Some(mut index) = match &msg.args[0] {
                            OscType::Double(v) => Some(v.max(0.0) as usize),
                            OscType::Float(v) => Some(v.max(0.0) as usize),
                            OscType::Int(v) => Some(*v.max(&0) as usize),
                            _ => None,
                        } {
                            let mut page = 0;
                            if msg.args.len() > 1 {
                                if let Some(p) = match &msg.args[1] {
                                    OscType::Double(v) => Some(v.max(0.0) as usize),
                                    OscType::Float(v) => Some(v.max(0.0) as usize),
                                    OscType::Int(v) => Some(*v.max(&0) as usize),
                                    _ => None,
                                } {
                                    page = p
                                } else {
                                    eprintln!("invalid 2nd arg for set view select");
                                    return;
                                }
                            }

                            //clamp
                            let ctx = self.sm.context();
                            let cnt = ctx.param_view_count();
                            if cnt > 0 {
                                index = index.min(cnt - 1);
                                let pages = ctx.view_param_pages(index);
                                if pages > 0 {
                                    page = page.min(pages - 1);
                                    self.handle_event(Events::SetViewSelected((index, page)))
                                        .await;
                                }
                            }
                        }
                    }
                }
                SET_VIEW_PAGE_DISPLAY => {
                    if msg.args.len() == 1 {
                        if let Some(index) = match &msg.args[0] {
                            OscType::Double(v) => Some(v.max(0.0) as usize),
                            OscType::Float(v) => Some(v.max(0.0) as usize),
                            OscType::Int(v) => Some(*v.max(&0) as usize),
                            _ => None,
                        } {
                            self.handle_event(Events::SetViewPageSelected(index)).await;
                        }
                    }
                }
                _ => {
                    if let Some(index) = self.param_lookup.get(&msg.addr) {
                        if let Some(param) = self.params.get_mut(*index) {
                            if msg.args.len() == 1 {
                                //ignore, we wait for normalized
                                match &msg.args[0] {
                                    OscType::Double(v) => param.update_f64(*v),
                                    OscType::Float(v) => param.update_f64(*v as f64),
                                    OscType::Int(v) => param.update_f64(*v as f64),
                                    OscType::String(v) => param.update_s(v),
                                    _ => (),
                                };
                            }
                        }
                    } else if let Some(index) = self.param_norm_lookup.get(&msg.addr) {
                        if let Some(param) = self.params.get_mut(*index) {
                            if msg.args.len() == 1 {
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
                                    //TODO throttle?

                                    let mut updates = Vec::new();
                                    //see if this param is visible, render it if so
                                    for (location, index) in self.visible_params.iter().enumerate()
                                    {
                                        if let Some(param) = self.params.get(*index) {
                                            if sparce.0 == param.instance_index()
                                                && sparce.1 == param.index()
                                            {
                                                self.render_param(&param, location);
                                                updates.push(location);
                                            }
                                        }
                                    }
                                    for location in updates {
                                        self.handle_event(Events::VisibleParamUpdated(location))
                                            .await;
                                    }
                                }
                            }
                        }
                    } else if let Some((index, name)) = self.dataref_lookup.get(&msg.addr) {
                        if msg.args.len() == 1 {
                            let mapping = match &msg.args[0] {
                                OscType::String(v) => {
                                    if v.len() > 0 {
                                        Some(v.clone())
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            };
                            if let Some(inst) = self.instances.get_mut(*index) {
                                if let Some(d) = inst.dataref_mappings_mut().get_mut(name) {
                                    *d.mapping_mut() = mapping;
                                    self.handle_event(Events::DatarefMappingChanged).await;
                                }
                            }
                        }
                    } else if let Some((index, name)) = self.dataref_meta_lookup.get(&msg.addr) {
                        if msg.args.len() == 1 {
                            let meta = match &msg.args[0] {
                                OscType::String(v) => {
                                    serde_json::from_str(v).unwrap_or(serde_json::Value::Null)
                                }
                                _ => serde_json::Value::Null,
                            };
                            if let Some(inst) = self.instances.get_mut(*index) {
                                if let Some(d) = inst.dataref_mappings_mut().get_mut(name) {
                                    if d.set_meta(&meta) {
                                        self.handle_event(Events::DatarefVisibleChanged).await;
                                    }
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

    pub async fn handle_midi(&mut self, bytes: &[u8]) -> bool {
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
        self.exit
    }

    fn update_common(&mut self, common: CommonContext) {
        self.sm.context_mut().update_common(common.clone());
        self.viewsm.context_mut().update_common(common.clone());
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

    fn do_once<F: Fn(&mut Self)>(&mut self, line: u32, func: F) {
        if self.line_token != line {
            func(self);
        }
        self.line_token = line;
    }

    fn render_buttons<const N: usize>(&mut self, btncolor: [(u8, MoveColor); N]) {
        let updated = HashMap::from(btncolor);
        let mut tracked = std::mem::take(&mut self.tracked_buttons);

        //check our tracked buttons and look for diffs
        for (btn, cur) in tracked.iter_mut() {
            let mut v = MoveColor::Black;
            if let Some(u) = updated.get(btn) {
                if cur == u {
                    continue;
                }
                v = *u;
            } else if cur == &MoveColor::Black {
                continue;
            }

            //update
            *cur = v;
            self.light_button(*btn, v as _);
        }
        std::mem::swap(&mut tracked, &mut self.tracked_buttons);
    }

    fn render_param_views(&mut self, frame: &mut ratatui::Frame) {
        use view::States;
        let s = self.viewsm.state().clone();
        match s {
            States::ParamViewMenu(selected) => {
                self.do_once(line!(), |s| {
                    s.clear_visible_params();
                    s.render_buttons([(MENU_MIDI, MoveColor::LightGray)]);
                });
                render_menu(
                    frame,
                    Some("Param Views"),
                    self.param_view_names.as_slice(),
                    default_indicator,
                    all_enabled,
                    selected,
                    None,
                );
            }
            States::ViewParams(state) => {
                self.do_once(line!(), |s| {
                    s.render_buttons([
                        (MENU_MIDI, MoveColor::LightGray),
                        (BACK_MIDI, MoveColor::LightGray),
                    ]);
                });

                let index = state.index;
                let page = state.page;
                let focused = state.focused.clone();

                //TODO how to compute this only when states change?
                if let Some((name, params)) = self
                    .param_view_names
                    .iter()
                    .zip(self.param_view_params.iter())
                    .skip(index)
                    .next()
                {
                    let offset = page * PARAM_PAGE_SIZE;

                    self.visible_params = params
                        .iter()
                        .skip(offset)
                        .take(PARAM_PAGE_SIZE)
                        .map(|i| *i)
                        .collect();

                    let pages = self.context().view_param_pages(index);
                    let title = format!("View: {}", name);

                    let mut focus: Option<ParamFocus> = None;
                    if let Some(focused) = focused {
                        let pindex = offset + focused;
                        if let Some(pindex) = params.get(pindex) {
                            if let Some(param) = self.params.get(*pindex) {
                                let label = format!(
                                    "inst: {} - {}",
                                    param.instance_index(),
                                    param.display_name(),
                                );
                                let value = param.render_value();
                                let norm = param.norm_prefer_pending();

                                focus = Some(ParamFocus { label, value, norm });
                            }
                        }
                    }
                    render_param_page(frame, &title, focus, page, pages);
                } else {
                    let title = format_title("Error");
                    let content = vec![Line::default(), Line::from("Empty View").centered()];
                    let paragraph = Paragraph::new(content).alignment(Alignment::Center);

                    let layout = titled_layout(frame.area());
                    frame.render_widget(title, layout[0]);
                    frame.render_widget(paragraph, layout[1]);
                }
            }
        }
    }

    fn render_main(&mut self, frame: &mut ratatui::Frame) {
        let state = self.sm.state().clone();

        let setup_common = |line: u32, s: &mut Self| {
            s.do_once(line, |s| {
                s.clear_visible_params();
                s.render_buttons([
                    (MENU_MIDI, MoveColor::LightGray),
                    (BACK_MIDI, MoveColor::LightGray),
                ]);
            });
        };

        match state {
            States::Menu(selected) => {
                setup_common(line!(), self);
                let indicator = |index: usize| -> &'static char {
                    let ctx = self.context();
                    match index {
                        TEMPO_INDEX | ABOUT_INDEX => ITEM_INDICATOR,
                        PATCHER_PARAMS_INDEX if ctx.instances_count(InstSelType::Params) < 2 => {
                            ITEM_INDICATOR
                        }
                        PATCHER_DATA_INDEX if ctx.instances_count(InstSelType::Datarefs) < 2 => {
                            ITEM_INDICATOR
                        }
                        _ => SUB_MENU_INDICATOR,
                    }
                };

                let enabled = |index: usize| -> bool {
                    let ctx = self.context();
                    match index {
                        PATCHER_PARAMS_INDEX => ctx.instances_count(InstSelType::Params) > 0,
                        PATCHER_DATA_INDEX => ctx.instances_count(InstSelType::Datarefs) > 0,
                        SETS_INDEX => ctx.sets_count() > 0,
                        SET_PRESETS_INDEX => ctx.set_presets_count() > 0,
                        _ => true,
                    }
                };

                render_menu(frame, None, &MENU_ITEMS, indicator, enabled, selected, None);
            }
            States::TempoEditor => {
                setup_common(line!(), self);

                let title = format_title("Tempo");
                let bpm = format!("{:.1} BPM", self.bpm).to_string();
                let content = vec![Line::default(), Line::from(bpm).centered()];
                let paragraph = Paragraph::new(content).alignment(Alignment::Center);

                let layout = titled_layout(frame.area());
                frame.render_widget(title, layout[0]);
                frame.render_widget(paragraph, layout[1]);
            }
            States::About => {
                setup_common(line!(), self);

                let title = format_title("About");
                let version = self
                    .package_version
                    .clone()
                    .unwrap_or("unknown".to_string());
                let content = vec![
                    Line::from("package version:"),
                    Line::from(version), /*, Line::from("beta.cycling74.com") */
                ];
                let paragraph = Paragraph::new(content).alignment(Alignment::Center);

                let layout = titled_layout(frame.area());
                frame.render_widget(title, layout[0]);
                frame.render_widget(paragraph, layout[1]);
            }
            States::SetsList(selected) => {
                setup_common(line!(), self);
                render_menu(
                    frame,
                    Some("Load Graph"),
                    self.set_names.as_slice(),
                    default_indicator,
                    all_enabled,
                    selected,
                    self.set_current_index,
                );
            }
            States::SetPresetsList(selected) => {
                setup_common(line!(), self);
                render_menu(
                    frame,
                    Some("Load Graph Preset"),
                    self.set_preset_names.as_slice(),
                    default_indicator,
                    all_enabled,
                    selected,
                    self.set_preset_loaded_index,
                );
            }
            States::PatcherInstances(entry) => {
                setup_common(line!(), self);
                let (title, items) = match entry.typ() {
                    InstSelType::Params => (&"Device Params", &self.patchers_params_instance_names),
                    InstSelType::Datarefs => {
                        (&"Device Data", &self.patchers_datarefs_instance_names)
                    }
                };
                render_menu(
                    frame,
                    Some(title),
                    items.as_slice(),
                    default_indicator,
                    all_enabled,
                    entry.selected(),
                    None,
                );
            }
            States::PatcherParams(state) => {
                //setup_common(line!(), self);
                let index = state.index;
                let page = state.page;
                let focused = state.focused.clone();

                let pages = self.context().instance_param_pages(index);

                //TODO how to compute this only when states change?
                let mut focus: Option<ParamFocus> = None;
                if let Some(instance) = self.instance_params.get(index) {
                    let offset = page * PARAM_PAGE_SIZE;
                    self.visible_params = instance
                        .iter()
                        .skip(offset)
                        .take(PARAM_PAGE_SIZE)
                        .map(|i| *i)
                        .collect();
                    if let Some(focused) = focused {
                        let pindex = offset + focused;
                        if let Some(pindex) = instance.get(pindex) {
                            if let Some(param) = self.params.get(*pindex) {
                                let label = param.display_name().to_string();
                                let value = param.render_value();
                                let norm = param.norm_prefer_pending();
                                focus = Some(ParamFocus { label, value, norm });
                            } else {
                                //eprintln!("cannot get param at {}", *pindex);
                            }
                        } else {
                            //eprintln!("cannot get pinstance {}", pindex);
                        }
                    }
                } else {
                    //self.visible_params.clear();
                }

                let name = self.patchers_params_instance_names.get(index).unwrap();
                let title = format!("{} Params", name);

                render_param_page(frame, &title, focus, page, pages);
            }
            States::PatcherDatarefs(entry) => {
                setup_common(line!(), self);
                if let Some(inst) = self
                    .instances
                    .get(self.patchers_datarefs_instance_indexes[entry.instance()])
                {
                    let name = self
                        .patchers_datarefs_instance_names
                        .get(entry.instance())
                        .unwrap();
                    let title = format!("{} Data", name);
                    let items: Vec<String> = inst.visible_datarefs().clone();

                    render_menu(
                        frame,
                        Some(title.as_str()),
                        inst.visible_datarefs().as_slice(),
                        default_indicator,
                        all_enabled,
                        entry.selected(),
                        None,
                    );
                }
            }
            States::PatcherDatarefLoad(entry) => {
                setup_common(line!(), self);
                if let Some(inst) = self
                    .instances
                    .get(self.patchers_datarefs_instance_indexes[entry.dataref().instance()])
                {
                    let indicated = inst
                        .visible_datarefs()
                        .iter()
                        .skip(entry.dataref().selected())
                        .next()
                        .map(|key| {
                            let dr = inst.dataref_mappings().get(key).unwrap();
                            if let Some(filename) = dr.mapping() {
                                self.datafile_list
                                    .iter()
                                    .position(|item| item == filename)
                                    .map(|index| index + 1) //+ 1 because of (unload) being first item
                                    .unwrap_or(0)
                            } else {
                                0
                            }
                        });
                    render_menu(
                        frame,
                        Some("Load File"),
                        self.datafile_menu.as_slice(),
                        default_indicator,
                        all_enabled,
                        entry.selected(),
                        indicated,
                    );
                }
            }
            _ => (), //TODO
        }
    }

    pub fn render(&mut self, frame: &mut ratatui::Frame) {
        use top::States;

        let state = self.topsm.state().clone();
        match state {
            States::Init => {
                self.do_once(line!(), |s| {
                    s.render_buttons([
                        (MENU_MIDI, MoveColor::LightGray),
                        (BACK_MIDI, MoveColor::LightGray),
                    ]);
                });

                use pad::PadStr;
                use std::collections::VecDeque;
                let w = frame.area().width as usize;
                let cnt = (frame.count() / ANIMATION_FRAME_DIV) % w;

                let mut text: Text = Default::default();

                let heading = "RNBO on Move!".pad_to_width(w);
                let (s, e) = heading.split_at(cnt);
                let s = e.to_string() + s;
                let mut line: VecDeque<char> = s.chars().collect();
                let e = if self.has_all_capabilities { 4 } else { 2 };
                for _ in 0..e {
                    text.push_line(Line::from(line.iter().collect::<String>()));
                    line.rotate_left(1);
                }
                if !self.has_all_capabilities {
                    text.push_line(Line::from("REDUCED"));
                    text.push_line(Line::from("CAPABILITIES"));
                }

                frame.render_widget(Paragraph::new(text.centered()).centered(), frame.area());
            }
            States::LaunchMove => {
                self.do_once(line!(), |s| {
                    s.render_buttons([]);
                });
                frame.render_widget(
                    Paragraph::new(Text::from("Launching Move").centered()).centered(),
                    frame.area(),
                );
            }
            States::PowerOff => {
                self.do_once(line!(), |s| {
                    s.render_buttons([]);
                });
                frame.render_widget(
                    Paragraph::new(Text::from("Powering Down").centered()).centered(),
                    frame.area(),
                );
            }
            States::PromptExit(selected) => {
                let can_exit = !self.child_process_error.is_some();
                self.do_once(line!(), |s| {
                    s.clear_visible_params();
                    if can_exit {
                        s.render_buttons([(BACK_MIDI, MoveColor::LightGray)]);
                    } else {
                        s.render_buttons([]);
                    }
                });

                render_menu(
                    frame,
                    Some(&"Exit"),
                    &EXIT_MENU,
                    default_indicator,
                    all_enabled,
                    selected,
                    None,
                );
            }
            States::VolumeEditor(_) => {
                self.do_once(line!(), |s| {
                    s.clear_visible_params();
                    s.render_buttons([
                        (BACK_MIDI, MoveColor::LightGray),
                        (MENU_MIDI, MoveColor::LightGray),
                    ]);
                });

                let title = format_title("Volume");
                let volume = format!("{:.2}", self.volume());
                let content = vec![Line::default(), Line::from(volume).centered()];
                let paragraph = Paragraph::new(content).alignment(Alignment::Center);

                let layout = titled_layout(frame.area());
                frame.render_widget(title, layout[0]);
                frame.render_widget(paragraph, layout[1]);
            }
            States::DisplayChildProcessError => {
                self.do_once(line!(), |s| {
                    s.clear_visible_params();
                    s.render_buttons([]);
                });

                let title = format_title("Crashed");

                let name = self.child_process_error.as_ref().unwrap().0.clone();
                let p = std::path::Path::new(name.as_str());
                let prog = p.file_name().unwrap().to_str().unwrap();
                let content = vec![
                    Line::from(prog),
                    Line::from("please report"),
                    Line::from("then hit power"),
                ];
                let paragraph = Paragraph::new(content).alignment(Alignment::Center);

                let layout = titled_layout(frame.area());
                frame.render_widget(title, layout[0]);
                frame.render_widget(paragraph, layout[1]);
            }
            States::Main => self.render_main(frame),
            States::ParamViews => self.render_param_views(frame),
        }
    }

    async fn handle_event(&mut self, e: Events) {
        let top_last = self.topsm.state().clone();
        let top_trans = self.topsm.process_event(e).is_some();
        let top_cur = self.topsm.state().clone();

        if top_trans {
            use top::States;
            match top_cur {
                States::LaunchMove => {
                    self.exit = true;
                }
                States::PowerOff => {
                    //XXX add time to display this?
                    self.send_power_cmd(PowerCommand::PowerOff);
                }
                //transitions
                States::Main | States::ParamViews => self.clear_visible_params(),
                _ => (),
            }
        }

        //if we're coming out of volume into a parameter editor for instance, we want to
        //know what we've touched
        let mut touch = false;

        //pass thru some events that always need to get thru
        match e {
            Events::EncTouch(e) if e < 8 => touch = true,
            Events::SetNamesChanged
            | Events::SetPresetNamesChanged
            | Events::SetCurrentChanged
            | Events::SetPresetLoadedChanged
                if top_cur != top::States::Main =>
            {
                let _ = self.sm.process_event(e);
            }
            Events::SetViewListChanged if top_cur != top::States::ParamViews => {
                let _ = self.viewsm.process_event(e);
            }
            _ => (),
        };

        //println!("top state {:?}", self.topsm.state());

        match top_cur {
            top::States::Main => {
                if touch || !top_trans {
                    let _ = self.sm.process_event(e);
                }
            }
            top::States::ParamViews => {
                match e {
                    Events::SetViewSelected(_) | Events::SetViewPageSelected(_) => {
                        touch = true; //hack to pass event thru
                    }
                    _ => (),
                };

                if touch || !top_trans {
                    let _ = self.viewsm.process_event(e);
                }
            }
            _ => (),
        };
    }

    fn render_param(&self, param: &Param, location: usize) {
        let cap = 0.96;
        let v = param.norm_prefer_pending();

        //TODO get from metdata?
        let color = Srgb::new(1.0, 1.0, 1.0).darken(cap - v * cap).into_format();

        for m in led_color(location as _, &color) {
            let _ = self.midi_out_queue.send(m);
        }
    }

    fn render_param_at(&self, index: usize, location: usize) {
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

    async fn offset_param(&mut self, index: usize, offset: isize) {
        if let Some(param) = self.params.get_mut(index) {
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

    pub async fn process_cmds(&mut self) {
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
                            self.offset_param(*index, offset).await;
                        }
                    }
                    //self.render_param(instance, param);
                }
                Cmd::OffsetViewParam {
                    view,
                    index,
                    offset,
                } => {
                    if let Some(params) = self.param_view_params.get(view) {
                        if let Some(index) = params.get(index) {
                            self.offset_param(*index, offset).await;
                        }
                    }
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

                Cmd::RenderVisibleParams => {
                    for i in 0..PARAM_PAGE_SIZE {
                        if let Some(index) = self.visible_params.get(i) {
                            self.render_param_at(*index, i);
                        } else {
                            self.clear_param(i);
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
                Cmd::UpdateDataFileList => {
                    self.datafile_list.clear();

                    if let Ok(entries) = std::fs::read_dir(DATFILE_DIR) {
                        for e in entries {
                            if let Ok(e) = e {
                                if let Some(f) = e.path().file_name() {
                                    let s = f.to_string_lossy().to_string();
                                    self.datafile_list.push(s);
                                }
                            }
                        }
                    };
                    self.datafile_list.sort();
                    self.datafile_menu = vec!["(empty)".into()];
                    self.datafile_menu
                        .append(&mut self.datafile_list.iter().map(|v| v.clone()).collect());

                    let mut common = self.sm.context().common();
                    common.datafile_count = self.datafile_menu.len();
                    self.update_common(common);
                }
                Cmd::LoadDataref((instance, datarefindex, fileindex)) => {
                    //0 == unload
                    let filename = if fileindex == 0 {
                        ""
                    } else {
                        if let Some(filename) = self.datafile_list.get(fileindex - 1) {
                            filename.as_str()
                        } else {
                            return;
                        }
                    };
                    if let Some(instance) = self.patchers_datarefs_instance_indexes.get(instance) {
                        if let Some(instance) = self.instances.get(*instance) {
                            if let Some(name) =
                                instance.visible_datarefs().iter().skip(datarefindex).next()
                            {
                                let addr =
                                    format!("/rnbo/inst/{}/data_refs/{}", instance.index(), name);
                                let msg = OscMessage {
                                    addr,
                                    args: vec![OscType::String(filename.to_string())],
                                };
                                self.send_osc(msg).await;
                            }
                        }
                    }
                }
                Cmd::ReportViewParamPage(index, page) => {
                    let msg = OscMessage {
                        addr: SET_VIEW_DISPLAY.to_string(),
                        args: vec![OscType::Int(index as _), OscType::Int(page as _)],
                    };
                    self.send_osc(msg).await;
                }
            }
        }
    }

    fn clear_visible_params(&mut self) {
        if self.visible_params.len() > 0 {
            self.visible_params.clear();
            self.clear_params();
        }
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

impl Drop for StateController {
    fn drop(&mut self) {
        if let Ok(file) = std::fs::File::create(&self.config_path) {
            let _ = serde_json::to_writer_pretty(file, &self.config);
        }
    }
}
