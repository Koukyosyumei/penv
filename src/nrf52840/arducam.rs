#[derive(Default)]
pub(super) struct ArduCam {
    image: Vec<u8>,
    frame: Vec<u8>,
    position: usize,
    captured: bool,
    command: Option<u8>,
    test: u8,
}

impl ArduCam {
    pub(super) fn load_image(&mut self, image: Vec<u8>) {
        self.image = image;
    }

    pub(super) fn reset(&mut self) {
        *self = Self {
            image: std::mem::take(&mut self.image),
            ..Self::default()
        };
    }

    pub(super) fn release(&mut self) {
        self.command = None;
    }

    pub(super) fn transmit(&mut self, value: u8) -> u8 {
        let Some(command) = self.command else {
            self.command = Some(value);
            return 0;
        };
        if command & 0x80 != 0 {
            match command & 0x7f {
                0 => self.test = value,
                4 => {
                    if value & 1 != 0 {
                        self.captured = false;
                    }
                    if value & 2 != 0 {
                        self.frame.clone_from(&self.image);
                        self.captured = true;
                        self.position = 0;
                    }
                }
                _ => {}
            }
            return 0;
        }
        match command {
            0 => self.test,
            0x3c => {
                if let Some(&byte) = self.frame.get(self.position) {
                    self.position += 1;
                    byte
                } else {
                    0
                }
            }
            0x41 => u8::from(self.captured) << 3,
            0x42..=0x44 => (self.frame.len() >> ((command - 0x42) * 8)) as u8,
            _ => 0,
        }
    }
}

#[derive(Default)]
pub(super) struct Ov2640 {
    sensor_bank: bool,
    register: u8,
    addressed: bool,
    image_mode: u8,
    zoom: [u8; 3],
    com7: u8,
}

impl Ov2640 {
    pub(super) fn start_write(&mut self) {
        self.addressed = false;
    }

    pub(super) fn write(&mut self, byte: u8) {
        if !self.addressed {
            self.register = byte;
            self.addressed = true;
            return;
        }
        match (self.sensor_bank, self.register) {
            (_, 0xff) => self.sensor_bank = byte & 1 != 0,
            (true, 0x12) if byte & 0x80 != 0 => *self = Self::default(),
            (true, 0x12) => self.com7 = byte & 0x70,
            (false, 0xda) => self.image_mode = byte,
            (false, 0x5a..=0x5c) => self.zoom[usize::from(self.register - 0x5a)] = byte,
            _ => {}
        }
    }

    pub(super) fn read(&self) -> u8 {
        match (self.sensor_bank, self.register) {
            (_, 0xff) => self.sensor_bank.into(),
            (true, 0x12) => self.com7,
            (false, 0xda) => self.image_mode,
            (false, 0x5a..=0x5c) => self.zoom[usize::from(self.register - 0x5a)],
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_holds_the_captured_frame_after_clearing_done() {
        let mut camera = ArduCam::default();
        camera.load_image(vec![1, 2, 3]);
        camera.transmit(0x42);
        assert_eq!(camera.transmit(0), 0);
        camera.release();
        camera.transmit(0x84);
        camera.transmit(2);
        camera.release();
        camera.transmit(0x41);
        assert_eq!(camera.transmit(0), 8);
        camera.release();
        camera.transmit(0x84);
        camera.transmit(1);
        camera.release();
        camera.transmit(0x41);
        assert_eq!(camera.transmit(0), 0);
        camera.release();
        camera.transmit(0x42);
        assert_eq!(camera.transmit(0), 3);
        camera.release();
        camera.transmit(0x3c);
        let bytes = [(); 4].map(|()| camera.transmit(0));
        assert_eq!(bytes, [1, 2, 3, 0]);
    }
}
