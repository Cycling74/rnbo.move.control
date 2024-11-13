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

    pub fn bytes(&self) -> &[u8; 3] {
        &self.bytes
    }

    pub fn len(&self) -> usize {
        self.len
    }
}
