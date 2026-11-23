pub struct GpioLed {
    pub name: String,
    pub pin: u8,
    pub parent: Option<String>,
    invert: bool,
    pub state: bool,
    pub transitions: u64,
}

impl GpioLed {
    pub fn for_nrf(name: String, parent: Option<String>, invert: bool) -> Self {
        let mut led = Self::new(name, 0, invert);
        led.parent = parent;
        led
    }

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

    #[cfg(test)]
    pub fn is_on(&self) -> bool {
        self.state
    }

    pub fn sample(&mut self, levels: u16, report: bool) {
        self.sample_level(levels & (1 << self.pin) != 0, report);
    }

    pub fn sample_level(&mut self, level: bool, report: bool) {
        let state = level ^ self.invert;
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

#[derive(Default)]
pub struct GpioEndpoints(Vec<(u32, u8)>);

impl GpioEndpoints {
    pub fn connect(
        &mut self,
        receiver: u32,
        name: &str,
        pin: u8,
        input_count: u8,
    ) -> Result<(), String> {
        if input_count != 0 && pin >= input_count {
            return Err(format!(
                "Cannot connect {pin}th input of {name}; it has only {input_count} GPIO inputs."
            ));
        }
        let endpoint = (receiver, pin);
        if !self.0.contains(&endpoint) {
            self.0.push(endpoint);
        }
        Ok(())
    }

    pub fn disconnect_receiver(&mut self, receiver: u32) {
        self.0.retain(|&(port, _)| port != receiver);
    }

    pub fn as_slice(&self) -> &[(u32, u8)] {
        &self.0
    }
}

pub struct GpioButton {
    pub name: String,
    pub parent: Option<String>,
    pub endpoints: GpioEndpoints,
    pub invert: bool,
}

impl GpioButton {
    pub fn new(
        name: String,
        parent: Option<String>,
        endpoints: GpioEndpoints,
        invert: bool,
    ) -> Self {
        Self {
            name,
            parent,
            endpoints,
            invert,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GpioEndpoints;

    #[test]
    fn renode_gpio_endpoint_registration_and_input_bounds() {
        let mut unbounded = GpioEndpoints::default();
        assert!(unbounded.as_slice().is_empty());
        unbounded
            .connect(0x4800_0000, "MockReceiver", 2, 0)
            .unwrap();
        assert_eq!(unbounded.as_slice(), &[(0x4800_0000, 2)]);

        let mut bounded = GpioEndpoints::default();
        let receiver = "Antmicro.Renode.UnitTests.Mocks.MockReceiverConstrained";
        bounded.connect(0x4800_0400, receiver, 2, 5).unwrap();
        assert_eq!(bounded.as_slice(), &[(0x4800_0400, 2)]);
        assert_eq!(
            bounded.connect(0x4800_0400, receiver, 10, 5).unwrap_err(),
            "Cannot connect 10th input of Antmicro.Renode.UnitTests.Mocks.MockReceiverConstrained; it has only 5 GPIO inputs."
        );
        assert_eq!(bounded.as_slice(), &[(0x4800_0400, 2)]);
    }
}
