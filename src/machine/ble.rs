use std::io::Write;

use crate::{bus::CORTEX_M_STEP_HZ, nrf52840::AirFrame};

use super::{Machine, Stop};

// Both upstream samples set the global synchronization quantum to 10 µs.
const QUANTUM_STEPS: u64 = CORTEX_M_STEP_HZ / 100_000;
const HOST_RELEASE_STEP: u64 = CORTEX_M_STEP_HZ;

impl Machine {
    pub fn run_radio_pair(
        &mut self,
        peer: &mut Self,
        limit: u64,
        output: &mut impl Write,
    ) -> Result<(Stop, u64), String> {
        let timed_uart = self.uart_step_prefix;
        run_ble(
            [self, peer],
            ["primary", "peer"],
            (0, 1),
            [],
            timed_uart,
            limit,
            output,
        )
    }

    pub fn run_hci_radio_pair(
        &mut self,
        peer: &mut Self,
        controllers: &mut [Self; 2],
        limit: u64,
        output: &mut impl Write,
    ) -> Result<(Stop, u64), String> {
        let central_host_link = self.bus.attach_uart_link("uart1")?;
        let peer_host_link = peer.bus.attach_uart_link("uart1")?;
        let [central_controller, peer_controller] = controllers;
        let central_controller_link = central_controller.bus.attach_uart_link("uart1")?;
        let peer_controller_link = peer_controller.bus.attach_uart_link("uart1")?;
        let timed_uart = self.uart_step_prefix;
        run_ble(
            [self, peer, central_controller, peer_controller],
            [
                "primary",
                "peer",
                "central controller",
                "peripheral controller",
            ],
            (2, 3),
            [
                (0, 2, central_host_link, central_controller_link),
                (1, 3, peer_host_link, peer_controller_link),
            ],
            timed_uart,
            limit,
            output,
        )
    }
}

fn run_ble<const N: usize, const L: usize>(
    mut nodes: [&mut Machine; N],
    labels: [&str; N],
    radios: (usize, usize),
    uart_links: [(usize, usize, usize, usize); L],
    timed_uart: bool,
    limit: u64,
    output: &mut impl Write,
) -> Result<(Stop, u64), String> {
    nodes[radios.0].bus.attach_radio_medium()?;
    nodes[radios.1].bus.attach_radio_medium()?;
    for node in &mut nodes {
        node.report_uart_steps(false);
    }

    let mut uart: [Vec<u8>; N] = std::array::from_fn(|_| Vec::new());
    let mut radio: [Vec<AirFrame>; N] = std::array::from_fn(|_| Vec::new());
    let mut retired = 0;
    let mut step = 0;
    while step < limit {
        let end = (limit - step).min(QUANTUM_STEPS) + step;
        let first_active = if !uart_links.is_empty() && step < HOST_RELEASE_STEP {
            radios.0
        } else {
            0
        };
        for index in first_active..N {
            let node = &mut nodes[index];
            node.elapsed_steps = step;
            let has_radio = index == radios.0 || index == radios.1;
            let (stop, count) = node
                .run_steps(
                    end - step,
                    &mut uart[index],
                    has_radio.then_some(&mut radio[index]),
                )
                .map_err(|e| format!("{}: {e}", labels[index]))?;
            retired += count;
            flush_uart(
                &mut uart[index],
                labels[index],
                timed_uart.then_some(node.elapsed_steps),
                output,
            )?;
            if stop != Stop::Limit {
                return Ok((stop, retired));
            }
        }

        // Cross-machine I/O becomes visible after both sides reach the synchronization point.
        for (host, controller, host_uart, controller_uart) in uart_links {
            let (hosts, controllers) = nodes.split_at_mut(controller);
            hosts[host]
                .bus
                .exchange_uart_link(host_uart, &mut controllers[0].bus, controller_uart)
                .map_err(|e| format!("{} HCI UART: {e}", labels[host]))?;
        }
        for (sender, receiver) in [radios, (radios.1, radios.0)] {
            for frame in radio[sender].drain(..) {
                nodes[receiver]
                    .bus
                    .receive_radio_air(frame)
                    .map_err(|e| format!("{} RADIO: {e}", labels[receiver]))?;
            }
        }
        step = end;
    }
    Ok((Stop::Limit, retired))
}

fn flush_uart(
    bytes: &mut Vec<u8>,
    label: &str,
    step: Option<u64>,
    output: &mut impl Write,
) -> Result<(), String> {
    let mut consumed = 0;
    for line in bytes.split_inclusive(|&byte| byte == b'\n') {
        if line.last() != Some(&b'\n') {
            break;
        }
        if let Some(step) = step {
            write!(output, "[step {step}] ").map_err(|e| e.to_string())?;
        }
        write!(output, "{label}: ").map_err(|e| e.to_string())?;
        output.write_all(line).map_err(|e| e.to_string())?;
        consumed += line.len();
    }
    bytes.drain(..consumed);
    Ok(())
}
