#[derive(Clone, Copy)]
pub enum ButtonAction {
    Press,
    Release,
    Toggle,
}

impl ButtonAction {
    pub fn parse(action: &str) -> Option<Self> {
        match action {
            "Press" => Some(Self::Press),
            "Release" => Some(Self::Release),
            "Toggle" => Some(Self::Toggle),
            _ => None,
        }
    }
}

pub struct Button {
    pub name: String,
    pub pin: u8,
    pub inverted: bool,
    pressed: bool,
}

impl Button {
    pub fn new(name: String, pin: u8, inverted: bool) -> Self {
        Self {
            name,
            pin,
            inverted,
            pressed: false,
        }
    }

    pub fn apply(&mut self, action: ButtonAction) -> bool {
        self.pressed = match action {
            ButtonAction::Press => true,
            ButtonAction::Release => false,
            ButtonAction::Toggle => !self.pressed,
        };
        self.pressed ^ self.inverted
    }
}

pub struct Panel {
    pub bases: Vec<u32>,
    pub irq: Option<(usize, u8)>,
    buttons: Vec<Button>,
    led_pins: Vec<(String, u8)>,
    leds: u32,
    switches: u32,
    pressed: u32,
    pending: u32,
    enabled: u32,
}

impl Panel {
    pub const SIZE: u32 = 0x14;

    pub fn new(
        bases: Vec<u32>,
        irq: Option<(usize, u8)>,
        buttons: Vec<Button>,
        led_pins: Vec<(String, u8)>,
    ) -> Self {
        let mut panel = Self {
            bases,
            irq,
            buttons,
            led_pins,
            leds: 0,
            switches: 0,
            pressed: 0,
            pending: 0,
            enabled: 0,
        };
        for index in 0..panel.buttons.len() {
            panel.set_button(index, ButtonAction::Release);
        }
        panel
    }

    pub fn led_state(&self, name: &str) -> Option<bool> {
        let name = name.strip_prefix("sysbus.").unwrap_or(name);
        self.led_pins
            .iter()
            .find(|(path, _)| path == name)
            .map(|(_, pin)| self.leds & (1 << pin) != 0)
    }

    pub fn interrupt_pending(&self) -> bool {
        self.pending & self.enabled != 0
    }

    pub fn button_action(&mut self, name: &str, action: ButtonAction) -> Result<(), String> {
        let name = name.rsplit('.').next().unwrap_or(name);
        let index = self
            .buttons
            .iter()
            .position(|button| button.name == name)
            .ok_or_else(|| format!("unknown LiteX button {name:?}"))?;
        self.set_button(index, action);
        Ok(())
    }

    pub fn has_button(&self, name: &str) -> bool {
        self.buttons.iter().any(|button| button.name == name)
    }

    fn set_button(&mut self, index: usize, action: ButtonAction) {
        let button = &mut self.buttons[index];
        let level = button.apply(action);
        let mask = 1 << (button.pin & 31);
        if button.pin < 64 {
            self.switches = (self.switches & !mask) | (u32::from(level) * mask);
        } else {
            self.pressed = (self.pressed & !mask) | (u32::from(level) * mask);
            self.pending |= u32::from(level) * mask;
        }
    }

    pub fn read(&self, offset: u32) -> u32 {
        match offset {
            0x00 => self.leds,
            0x04 => self.switches,
            0x08 => self.pressed,
            0x0c => self.pending,
            0x10 => self.enabled,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32) {
        match offset {
            0x00 => self.leds = value,
            0x0c => self.pending &= !value,
            0x10 => self.enabled = value,
            _ => {}
        }
    }
}
