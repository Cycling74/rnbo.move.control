pub struct Midi {
    bytes: [u8; 3],
    len: usize,
}

#[allow(dead_code)]
impl Midi {
    pub fn new(v: &[u8]) -> Self {
        assert!(v.len() <= 3);
        let mut bytes = [0; 3];
        for (b, i) in bytes.iter_mut().zip(v) {
            *b = *i;
        }

        Self {
            bytes,
            len: v.len(),
        }
    }

    pub fn reset() -> Self {
        Self::new(&[0xFF])
    }

    pub fn cc(num: u8, val: u8, chan: u8) -> Self {
        Midi::new(&[0xb0u8 | chan, num, val])
    }

    pub fn note_on(num: u8, vel: u8, chan: u8) -> Self {
        Midi::new(&[0x90u8 | chan, num, vel])
    }

    pub fn note_off(num: u8, vel: u8, chan: u8) -> Self {
        Midi::new(&[0x80u8 | chan, num, vel])
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes[0..self.len]
    }

    pub fn len(&self) -> usize {
        self.len
    }
}
