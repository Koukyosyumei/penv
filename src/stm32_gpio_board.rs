pub struct GpioLed {
    pub name: String,
    pub pin: u8,
    pub parent: Option<String>,
    invert: bool,
    pub state: bool,
    pub transitions: u64,
}

impl GpioLed {
    pub fn new(name: String, pin: u8, invert: bool) -> Self {
        Self {
            name,
            pin,
            parent: None,
            invert,
            state: invert,
            transitions: 0,
        }
    }

    pub fn sample(&mut self, levels: u16, report: bool) {
        let state = (levels & (1 << self.pin) != 0) ^ self.invert;
        if state == self.state {
            return;
        }
        self.state = state;
        self.transitions = self.transitions.wrapping_add(1);
        if report {
            eprintln!("LED {} {}", self.name, if state { "on" } else { "off" });
        }
    }
}

pub struct GpioButton {
    pub name: String,
    pub parent: Option<String>,
    pub port: u32,
    pub pin: u8,
    pub invert: bool,
}

impl GpioButton {
    pub fn new(name: String, parent: Option<String>, port: u32, pin: u8, invert: bool) -> Self {
        Self {
            name,
            parent,
            port,
            pin,
            invert,
        }
    }
}
