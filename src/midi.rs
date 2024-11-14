pub struct Midi {
    bytes: [u8; 3],
    len: usize,
}

impl Midi {
    pub fn new(v: &[u8]) -> Self {
        let mut bytes = [0; 3];
        bytes.copy_from_slice(v);

        if v.len() != 3 {
            println!("got midi that isn't 3 bytes: {:?}", v);
        }

        Self {
            bytes,
            len: v.len(),
        }
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

    pub fn bytes(&self) -> &[u8; 3] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.len
    }
}
