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
