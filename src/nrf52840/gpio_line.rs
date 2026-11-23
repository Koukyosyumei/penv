#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Endpoint<R> {
    pub receiver: R,
    pub input: u32,
}

pub(crate) struct GpioLine<R> {
    endpoints: Vec<Endpoint<R>>,
}

impl<R> Default for GpioLine<R> {
    fn default() -> Self {
        Self {
            endpoints: Vec::new(),
        }
    }
}

impl<R: Eq> GpioLine<R> {
    pub fn connect(
        &mut self,
        receiver: R,
        name: &str,
        inputs: u32,
        input: u32,
    ) -> Result<(), String> {
        if inputs != 0 && input >= inputs {
            return Err(format!(
                "Cannot connect {input}th input of {name}; it has only {inputs} GPIO inputs."
            ));
        }
        let endpoint = Endpoint { receiver, input };
        if !self.endpoints.contains(&endpoint) {
            self.endpoints.push(endpoint);
        }
        Ok(())
    }

    pub fn endpoints(&self) -> &[Endpoint<R>] {
        &self.endpoints
    }
}

pub(crate) struct GpioRoutes<R> {
    lines: std::collections::BTreeMap<(u32, u8), GpioLine<R>>,
}

impl<R> Default for GpioRoutes<R> {
    fn default() -> Self {
        Self {
            lines: std::collections::BTreeMap::new(),
        }
    }
}

impl<R: Eq> GpioRoutes<R> {
    pub fn line_mut(&mut self, source: u32, pin: u8) -> &mut GpioLine<R> {
        self.lines.entry((source, pin)).or_default()
    }

    pub fn lines(&self, source: u32) -> impl Iterator<Item = (u8, &GpioLine<R>)> {
        self.lines
            .range((source, 0)..=(source, u8::MAX))
            .map(|(&(_, pin), line)| (pin, line))
    }

    pub fn unregister_receiver(&mut self, receiver: &R) {
        for line in self.lines.values_mut() {
            line.endpoints
                .retain(|endpoint| &endpoint.receiver != receiver);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECEIVER: &str = "Antmicro.Renode.UnitTests.Mocks.MockReceiverConstrained";

    #[test]
    fn upstream_gpio_propagates_connected_endpoint() {
        let mut line = GpioLine::default();
        line.connect(7, "MockReceiver", 0, 2).unwrap();
        assert_eq!(
            line.endpoints(),
            &[Endpoint {
                receiver: 7,
                input: 2
            }]
        );
    }

    #[test]
    fn upstream_gpio_endpoints_are_empty_when_not_connected() {
        assert!(GpioLine::<u32>::default().endpoints().is_empty());
    }

    #[test]
    fn upstream_gpio_connects_bounded_receiver() {
        GpioLine::default().connect(7, RECEIVER, 5, 2).unwrap();
    }

    #[test]
    fn upstream_gpio_rejects_illegal_input_number() {
        let mut line = GpioLine::default();
        assert_eq!(
            line.connect(7, RECEIVER, 5, 10).unwrap_err(),
            "Cannot connect 10th input of Antmicro.Renode.UnitTests.Mocks.MockReceiverConstrained; it has only 5 GPIO inputs."
        );
        assert!(line.endpoints().is_empty());
    }
}

#[cfg(test)]
mod detach_tests {
    use super::*;

    fn register_fixture(count: u8) -> GpioRoutes<u32> {
        let mut routes = GpioRoutes::default();
        for pin in 0..count {
            let receiver = (u32::from(pin) + 1) * 0x10;
            routes
                .line_mut(0, pin)
                .connect(receiver, "MockReceiver", 0, u32::from(pin) + 1)
                .unwrap();
        }
        routes
    }

    #[test]
    fn upstream_gpio_detach_lists_all_numbered_connections() {
        let routes = register_fixture(1);
        assert_eq!(routes.lines(0).count(), 1);
    }

    #[test]
    fn upstream_gpio_detach_unregisters_numbered_receiver() {
        let mut routes = register_fixture(1);
        routes.unregister_receiver(&0x10);
        assert!(routes.lines(0).next().unwrap().1.endpoints().is_empty());
    }

    #[test]
    fn upstream_gpio_detach_preserves_other_numbered_receivers() {
        let mut routes = register_fixture(3);
        routes.unregister_receiver(&0x10);
        let mut lines = routes.lines(0);
        assert!(lines.next().unwrap().1.endpoints().is_empty());
        assert!(!lines.next().unwrap().1.endpoints().is_empty());
        assert!(!lines.next().unwrap().1.endpoints().is_empty());
    }

    #[test]
    fn upstream_gpio_detach_unregisters_irq_receiver() {
        let mut routes = GpioRoutes::default();
        routes
            .line_mut(0x10, 0)
            .connect(0, "MockReceiver", 0, 1)
            .unwrap();
        routes.unregister_receiver(&0);
        assert!(routes.lines(0x10).next().unwrap().1.endpoints().is_empty());
    }
}
