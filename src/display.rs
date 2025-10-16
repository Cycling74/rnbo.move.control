use embedded_graphics_core::{draw_target::DrawTarget, pixelcolor::BinaryColor, prelude::*};

const HEADER_LEN: usize = 8;
pub const DISPLAY_BYTES: usize = 1024;

pub const DISPLAY_WIDTH: u32 = 128;
pub const DISPLAY_HEIGHT: u32 = 64;
pub const DISPLAY_WIDTH_M1: u32 = 128 - 1;
pub const DISPLAY_HEIGHT_M1: u32 = 64 - 1;

pub const BUFFER_LEN: usize = HEADER_LEN + DISPLAY_BYTES;

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
enum DrawMode {
    Set,
    Sum,
    Xor,
}

pub struct DrawCommand {
    pub data: [u8; BUFFER_LEN],
}

pub struct MoveDisplay {
    framebuffer: [u8; BUFFER_LEN],
    dirty: bool,
    mode: DrawMode,
}

impl MoveDisplay {
    pub fn new() -> Self {
        //create buffer and add header
        let mut framebuffer = [0; BUFFER_LEN];
        for (i, b) in [b'M', b'O', b'V', b'E', b'D', b'I', b'S', b'P']
            .iter()
            .zip(framebuffer.iter_mut())
        {
            *b = *i;
        }

        Self {
            framebuffer,
            dirty: true,
            mode: DrawMode::Set,
        }
    }

    pub fn draw_if<F: FnMut(&[u8; BUFFER_LEN])>(&mut self, mut f: F) {
        if self.dirty {
            f(&self.framebuffer);
            self.dirty = false;
        }
    }

    pub fn with_summing<T, F: FnOnce(&mut Self) -> T>(&mut self, f: F) -> T {
        let mode = self.mode;
        self.mode = DrawMode::Sum;
        let v = f(self);
        self.mode = mode;
        v
    }

    pub fn with_xor<T, F: FnOnce(&mut Self) -> T>(&mut self, f: F) -> T {
        let mode = self.mode;
        self.mode = DrawMode::Xor;
        let v = f(self);
        self.mode = mode;
        v
    }

    pub fn framebuffer(&self) -> &[u8; BUFFER_LEN] {
        &self.framebuffer
    }

    /*
    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn buffer(&self) -> [u8; BUFFER_LEN] {
        self.framebuffer.clone()
    }
    */
}
impl OriginDimensions for MoveDisplay {
    fn size(&self) -> Size {
        Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT)
    }
}

fn render_set(color: BinaryColor, bit: u8, cur: u8) -> u8 {
    match color {
        BinaryColor::On => cur | bit,
        BinaryColor::Off => cur & !bit,
    }
}

fn render_sum(color: BinaryColor, bit: u8, cur: u8) -> u8 {
    match color {
        BinaryColor::On => cur | bit,
        BinaryColor::Off => cur,
    }
}

fn render_xnor(color: BinaryColor, bit: u8, cur: u8) -> u8 {
    match color {
        BinaryColor::On => cur ^ bit,
        BinaryColor::Off => cur,
    }
}

impl DrawTarget for MoveDisplay {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        self.dirty = true;

        let render: fn(BinaryColor, u8, u8) -> u8 = match self.mode {
            DrawMode::Set => render_set,
            DrawMode::Sum => render_sum,
            DrawMode::Xor => render_xnor,
        };

        for Pixel(coord, color) in pixels.into_iter() {
            if let Ok((x @ 0..=DISPLAY_WIDTH_M1, y @ 0..=DISPLAY_HEIGHT_M1)) = coord.try_into() {
                let byte: usize = (x + y / 8 * DISPLAY_WIDTH) as usize + HEADER_LEN;
                let bit: u8 = 1 << (y as usize % 8);
                self.framebuffer[byte] = render(color, bit, self.framebuffer[byte]);
            }
        }

        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.dirty = true;
        self.framebuffer[HEADER_LEN..].fill(match color {
            BinaryColor::On => 0xFF,
            BinaryColor::Off => 0x00,
        });
        Ok(())
    }
}
