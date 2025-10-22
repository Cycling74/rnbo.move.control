use {
    crate::display::MoveDisplay,
    embedded_graphics::{
        image::{Image, ImageRaw},
        pixelcolor::BinaryColor,
        prelude::*,
    },
};

const HEADER_BYTES: usize = 32;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum Format {
    UserImage,
    Float32Buffer,
    Float64Buffer,
}

#[derive(Clone)]
pub struct ViewData {
    view_index: usize,
    param_view_name: Option<String>,

    view_z: Option<i8>,
    view_hidden: bool,
    view_name: Option<String>,
    shm_name: Option<String>,
    view_xor: bool,
}

struct UserViewLayer {
    shm_name: Option<String>,
    shm: Option<psx_shm::Shm>,

    format: Format,
    z: i8,

    dirty: bool,
    rendering: Vec<u8>,
    hidden: bool,
    do_xor: bool,

    buffer: String,

    channels: usize,
    samplerate: usize,

    param_view_name: Option<String>,

    offset: Point,
    width: u32,
    height: u32,
}

pub struct UserView {
    name: Option<String>,
    param_view_name: Option<String>,
    layers: Vec<UserViewLayer>,
}

impl ViewData {
    pub fn new(
        view_index: usize,
        param_view_name: Option<String>,

        view_z: Option<i8>,
        view_hidden: bool,
        view_name: Option<String>,
        shm_name: Option<String>,
        view_xor: bool,
    ) -> Self {
        Self {
            view_index,
            param_view_name,

            view_z,
            view_hidden,
            view_name,
            shm_name,
            view_xor,
        }
    }

    pub fn view_index(&self) -> usize {
        self.view_index
    }

    pub fn param_view_name(&self) -> &Option<String> {
        &self.param_view_name
    }

    pub fn view_z(&self) -> Option<i8> {
        self.view_z
    }

    pub fn view_hidden(&self) -> bool {
        self.view_hidden
    }

    pub fn view_name(&self) -> &Option<String> {
        &self.view_name
    }

    pub fn shm_name(&self) -> &Option<String> {
        &self.shm_name
    }

    pub fn view_xor(&self) -> bool {
        self.view_xor
    }
}

impl UserView {
    pub fn new(name: Option<String>, param_view_name: Option<String>) -> Self {
        Self {
            name,
            param_view_name,
            layers: Vec::new(),
        }
    }

    pub fn name(&self) -> &Option<String> {
        &self.name
    }

    pub fn name_or_default(&self, index: usize) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("view {}", index))
    }

    pub fn set_name(&mut self, name: Option<String>) {
        self.name = name;
    }

    pub fn set_param_view_name(&mut self, name: Option<String>) {
        self.param_view_name = name;
    }

    pub fn param_view_name(&self) -> &Option<String> {
        &self.param_view_name
    }

    fn with_layer<F: FnOnce(&mut UserViewLayer)>(&mut self, buffer: &str, f: F) -> bool {
        for layer in self.layers.iter_mut() {
            if layer.buffer == buffer {
                f(layer);
                return true;
            }
        }
        false
    }

    fn with_layer_z<F: FnOnce(&mut UserViewLayer)>(&mut self, z: i8, f: F) {
        for layer in self.layers.iter_mut() {
            if layer.z == z {
                f(layer);
                break;
            }
        }
    }

    pub fn exit(&mut self) {
        for layer in self.layers.iter_mut() {
            layer.exit();
        }
    }

    pub fn empty(&self) -> bool {
        self.layers.len() == 0
    }

    pub fn set_layer_dirty(&mut self, z: i8) {
        self.with_layer_z(z, |l| l.dirty = true);
    }

    pub fn set_layer_hidden(&mut self, z: i8, hidden: bool) {
        self.with_layer_z(z, |l| l.hidden = hidden);
    }

    pub fn set_layer_xor(&mut self, z: i8, xor: bool) {
        self.with_layer_z(z, |l| l.do_xor = xor);
    }

    pub fn render(&mut self, display: &mut MoveDisplay) {
        for layer in self.layers.iter_mut() {
            layer.render(display);
        }
    }

    pub fn copy_shm(&mut self, other: &mut Self) {
        for layer in self.layers.iter_mut() {
            for other in other.layers.iter_mut() {
                if other.shm.is_some() && other.shm_name == layer.shm_name {
                    std::mem::swap(&mut layer.shm, &mut other.shm);
                    std::mem::swap(&mut layer.rendering, &mut other.rendering);
                    break;
                }
            }
        }
    }

    //update or add
    pub fn update_layers(&mut self, buffer: &str, view_data: &ViewData) {
        if let Some(view_name) = view_data.view_name() {
            self.set_name(Some(view_name.clone()));
        }

        if !self.with_layer(buffer, |layer| {
            layer.update(view_data);
        }) {
            self.layers.push(UserViewLayer::new(buffer, view_data));
        }
        self.layers.sort_by_key(|l| l.z);

        let mut param_view_name = None;
        for layer in self.layers.iter() {
            if let Some(name) = layer.param_view_name() {
                param_view_name = Some(name.clone());
                break;
            }
        }
        self.param_view_name = param_view_name;
    }

    pub fn remove_layer(&mut self, buffer: &str) -> bool {
        self.layers.retain(|l| l.buffer != buffer);
        self.empty()
    }

    pub fn clear_layers(&mut self) {
        self.layers.clear();
    }
}

struct FormatInfo {
    format: Format,
    channels: usize,
    samplerate: usize,
}

impl Default for FormatInfo {
    fn default() -> Self {
        Self {
            format: Format::UserImage,
            channels: 0,
            samplerate: 0,
        }
    }
}

fn get_format(shm_name: &str) -> FormatInfo {
    use std::str::FromStr;
    let mut format = Format::UserImage;
    let mut channels = 0;
    let mut samplerate = 0;
    let parts: Vec<&str> = shm_name.split("-").collect();
    if parts.len() >= 4 {
        match parts[1] {
            "u8" => format = Format::UserImage,
            "f32" => format = Format::Float32Buffer,
            "f64" => format = Format::Float64Buffer,
            _ => (),
        }
        if format != Format::UserImage {
            channels = usize::from_str(parts[2]).unwrap_or(0);
            samplerate = usize::from_str(parts[3]).unwrap_or(0);
        }
    }
    FormatInfo {
        format,
        channels,
        samplerate,
    }
}

fn render_waveform<
    T: num_traits::Float + num_traits::Zero + PartialOrd + num_traits::ToPrimitive,
>(
    rendered: &mut Vec<u8>,
    rows: usize,
    cols: usize,
    channels: usize,
    buffer: &[u8],
) {
    use num_traits::clamp;

    let col_bytes = cols / 8;
    let bytes = col_bytes * rows;

    rendered.resize(bytes as _, 0);
    rendered.fill(0);

    let len = buffer.len() / size_of::<T>();
    let data = unsafe {
        std::slice::from_raw_parts::<'_, T>(
            std::mem::transmute::<_, *const T>(buffer.as_ptr()),
            len,
        )
    };

    let mid_1 = rows / 2 - 1;
    let mid_1_t = T::from(mid_1).unwrap();

    //TODO chunk rendering??
    let frames = len / channels;
    if frames > 0 {
        let chunksize = len / cols as usize;
        for col in 0..cols {
            let start = col * chunksize;
            let cbyte = col / 8;
            let cbit = 7 - (col % 8);
            let mut max: T = T::zero();
            for v in data.iter().skip(start).take(chunksize) {
                max = v.abs().max(max);
            }

            let rows = clamp(mid_1_t * max, T::zero(), mid_1_t)
                .to_usize()
                .unwrap_or(0);
            let mask = 1 << cbit;
            for r in 0..rows {
                //positive from center
                let byte = cbyte + (mid_1 - 1 - r) * col_bytes;
                rendered[byte] = rendered[byte] | mask;

                //negative from center
                let byte = cbyte + (mid_1 + r) * col_bytes;
                rendered[byte] = rendered[byte] | mask;
            }
        }
    }
}

impl UserViewLayer {
    fn new(buffer: &str, view_data: &ViewData) -> Self {
        let shm_name = view_data.shm_name().clone();

        let format = if let Some(shm_name) = &shm_name {
            get_format(shm_name)
        } else {
            FormatInfo::default()
        };
        Self {
            shm_name,
            shm: None,

            format: format.format,
            z: view_data.view_z().unwrap_or(0),

            dirty: true,
            rendering: Vec::new(),
            hidden: view_data.view_hidden(),
            do_xor: view_data.view_xor(),

            buffer: buffer.to_owned(),

            channels: format.channels,
            samplerate: format.samplerate,

            param_view_name: view_data.param_view_name().clone(),

            offset: Point::new(0, 0),
            width: 0,
            height: 0,
        }
    }

    fn param_view_name(&self) -> &Option<String> {
        &self.param_view_name
    }

    fn update(&mut self, view_data: &ViewData) {
        self.z = view_data.view_z().unwrap_or(0);
        if self.shm_name != view_data.shm_name {
            self.rendering.clear();
            self.shm_name = view_data.shm_name.clone();
            self.shm = None;
            self.dirty = true;
        }
        self.hidden = view_data.view_hidden();
        self.do_xor = view_data.view_xor();
        self.param_view_name = view_data.param_view_name().clone();

        let format = if let Some(shm_name) = &self.shm_name {
            get_format(shm_name)
        } else {
            FormatInfo::default()
        };
        self.format = format.format; //XXX assert no change?
        self.samplerate = format.samplerate;
        self.channels = format.channels;
    }

    fn exit(&mut self) {
        //is there any more cleanup to do?
        self.shm = None;
    }

    fn render(&mut self, display: &mut MoveDisplay) {
        if !self.hidden {
            let height: u32 = display.size().height;
            let width: u32 = display.size().width;

            if self.dirty {
                if self.shm.is_none() {
                    if let Some(shm_name) = &self.shm_name {
                        if let Ok(shm) = psx_shm::Shm::open(
                            shm_name.as_str(),
                            rustix::shm::OFlags::RDWR,
                            rustix::shm::Mode::empty(),
                        ) {
                            self.shm = Some(shm);
                        } else {
                            return;
                        }
                    } else {
                        return;
                    }
                }
                if let Some(shm) = &mut self.shm {
                    if let Ok(mut map) = unsafe { shm.map(0) } {
                        let map = map.map();
                        let contents = map.as_mut();
                        match self.format {
                            Format::UserImage => {
                                if contents.len() > HEADER_BYTES {
                                    let flag = unsafe {
                                        std::sync::atomic::AtomicU8::from_ptr(contents.as_mut_ptr())
                                    };

                                    //dirty flag
                                    if flag.load(std::sync::atomic::Ordering::SeqCst) == 1 {
                                        //width and offset
                                        {
                                            let header = unsafe {
                                                std::slice::from_raw_parts::<'_, u32>(
                                                    std::mem::transmute::<_, *const u32>(
                                                        contents.as_ptr(),
                                                    ),
                                                    contents.len() / size_of::<u32>(),
                                                )
                                            };
                                            let w = header[1];
                                            let h = header[2];
                                            self.width = if w > 0 { w } else { width };
                                            self.height = if h > 0 { h } else { height };

                                            let header = unsafe {
                                                std::slice::from_raw_parts::<'_, i32>(
                                                    std::mem::transmute::<_, *const i32>(
                                                        contents.as_ptr(),
                                                    ),
                                                    contents.len() / size_of::<i32>(),
                                                )
                                            };
                                            self.offset = Point::new(header[3], header[4]);
                                        }

                                        //row is zero padded to nearest byte
                                        let row_bytes = (self.width / 8)
                                            + if self.width % 8 > 0 { 1 } else { 0 };
                                        let image_bytes = row_bytes * self.height;
                                        self.rendering = contents[HEADER_BYTES..]
                                            .iter()
                                            .take(image_bytes as _)
                                            .map(|v| *v)
                                            .collect();
                                        //clear dirty flag
                                        flag.store(0, std::sync::atomic::Ordering::SeqCst);
                                    }
                                }
                            }
                            Format::Float32Buffer if self.channels > 0 => {
                                self.width = width;
                                self.height = height;
                                //TODO chunk rendering??
                                render_waveform::<f32>(
                                    &mut self.rendering,
                                    height as _,
                                    width as _,
                                    self.channels,
                                    &contents,
                                );
                                self.dirty = false;
                            }
                            Format::Float64Buffer if self.channels > 0 => {
                                self.width = width;
                                self.height = height;
                                //TODO chunk rendering??
                                render_waveform::<f64>(
                                    &mut self.rendering,
                                    height as _,
                                    width as _,
                                    self.channels,
                                    &contents,
                                );
                                self.dirty = false;
                            }
                            _ => return,
                        }
                    }
                } else {
                    return;
                }
            }
            if self.rendering.len() > 0 && self.width > 0 {
                let image = ImageRaw::<BinaryColor>::new(self.rendering.as_slice(), self.width);
                let image = Image::new(&image, self.offset);
                if self.do_xor {
                    display.with_xor(|display| {
                        if image.draw(display).is_err() {
                            eprintln!("error drawing image");
                        }
                    });
                } else {
                    display.with_summing(|display| {
                        if image.draw(display).is_err() {
                            eprintln!("error drawing image");
                        }
                    });
                }
            }
        }
    }
}
