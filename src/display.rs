use embedded_graphics_core::{draw_target::DrawTarget, pixelcolor::BinaryColor, prelude::*};

const HEADER_LEN: usize = 8;
const DISPLAY_BYTES: usize = 1024;
const DISPLAY_WIDTH: u32 = 128;
const DISPLAY_HEIGHT: u32 = 64;

const DISPLAY_WIDTH_M1: u32 = 128 - 1;
const DISPLAY_HEIGHT_M1: u32 = 64 - 1;

const BUFFER_LEN: usize = HEADER_LEN + DISPLAY_BYTES;

pub struct MoveDisplay {
    framebuffer: [u8; BUFFER_LEN],
    dirty: bool,
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
        }
    }

    pub fn draw_if<F: FnMut(&[u8; BUFFER_LEN])>(&mut self, mut f: F) {
        if self.dirty {
            f(&self.framebuffer);
            self.dirty = false;
        }
    }
}
impl OriginDimensions for MoveDisplay {
    fn size(&self) -> Size {
        Size::new(DISPLAY_WIDTH, DISPLAY_HEIGHT)
    }
}

impl DrawTarget for MoveDisplay {
    type Color = BinaryColor;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels.into_iter() {
            if let Ok((x @ 0..=DISPLAY_WIDTH_M1, y @ 0..=DISPLAY_HEIGHT_M1)) = coord.try_into() {
                let byte: usize = (x + y / 8 * DISPLAY_WIDTH) as usize + HEADER_LEN;
                let bit: u8 = 1 << (y as usize % 8);
                match color {
                    BinaryColor::On => self.framebuffer[byte] |= bit,
                    BinaryColor::Off => self.framebuffer[byte] &= !bit,
                }
            }
        }

        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.framebuffer[HEADER_LEN..].fill(match color {
            BinaryColor::On => 0xFF,
            BinaryColor::Off => 0x00,
        });
        Ok(())
    }
}
