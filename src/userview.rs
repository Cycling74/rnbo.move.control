use {
    crate::display::MoveDisplay,
    embedded_graphics::{
        image::{Image, ImageRaw},
        pixelcolor::BinaryColor,
        prelude::*,
    },
};

const HEADER_BYTES: usize = 16;

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

impl UserViewLayer {
    fn new(buffer: &str, view_data: &ViewData) -> Self {
        use std::str::FromStr;
        let mut format = Format::UserImage;
        let mut channels = 0;
        let mut samplerate = 0;

        let shm_name = view_data.shm_name().clone();

        if let Some(shm_name) = &shm_name {
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
        }
        Self {
            shm_name,
            shm: None,

            format,
            z: view_data.view_z().unwrap_or(0),

            dirty: true,
            rendering: Vec::new(),
            hidden: view_data.view_hidden(),
            do_xor: view_data.view_xor(),

            buffer: buffer.to_owned(),

            channels,
            samplerate,

            param_view_name: view_data.param_view_name().clone(),
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
        }
        self.hidden = view_data.view_hidden();
        self.do_xor = view_data.view_xor();
        self.param_view_name = view_data.param_view_name().clone();

        //TODO assert format stays the same?
    }

    fn exit(&mut self) {
        //is there any more cleanup to do?
        self.shm = None;
    }

    fn render(&mut self, display: &mut MoveDisplay) {
        if !self.hidden {
            let width: u32 = display.size().width;
            let offset: Point = Point::new(0, 0);
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
                match self.format {
                    Format::UserImage => {
                        if let Some(shm) = &mut self.shm {
                            if let Ok(mut map) = unsafe { shm.map(0) } {
                                let map = map.map();
                                let contents = map.as_mut();
                                if contents.len() > HEADER_BYTES {
                                    let header = unsafe {
                                        std::sync::atomic::AtomicU8::from_ptr(contents.as_mut_ptr())
                                    };

                                    if header.load(std::sync::atomic::Ordering::SeqCst) == 1 {
                                        //dirty flag
                                        //header
                                        self.rendering =
                                            contents[HEADER_BYTES..].iter().map(|v| *v).collect();
                                        //TODO read width
                                        //clear dirty flag
                                        header.store(0, std::sync::atomic::Ordering::SeqCst);
                                    }
                                }
                            }
                        } else {
                            return;
                        }
                    }
                    Format::Float32Buffer | Format::Float64Buffer => {
                        //TODO
                        return;
                    }
                }
            }
            if self.rendering.len() > 0 {
                let image = ImageRaw::<BinaryColor>::new(self.rendering.as_slice(), width);
                let image = Image::new(&image, offset);
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
