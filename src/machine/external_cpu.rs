use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    bus::BusError,
    bus_translation::PeripheralHandle,
    dma_engine::{DmaBus, Width},
};

use super::Machine;

pub trait ExternalCpu: Send {
    fn architecture(&self) -> &str;
    fn run(&mut self, bus: &CpuBus);
}

struct ReadRequest {
    address: u64,
    reply: Sender<Result<u32, BusError>>,
}

pub struct CpuBus {
    requests: Sender<ReadRequest>,
    running: Arc<AtomicBool>,
}

impl CpuBus {
    /// Returns None when the machine pauses its external CPUs.
    pub fn read_doubleword(&self, address: u64) -> Option<Result<u32, BusError>> {
        if !self.running.load(Ordering::Acquire) {
            return None;
        }
        let (reply, result) = mpsc::channel();
        self.requests.send(ReadRequest { address, reply }).ok()?;
        result.recv().ok()
    }
}

pub(super) struct Registration {
    identity: PeripheralHandle,
    cpu: Box<dyn ExternalCpu>,
}

impl Machine {
    pub fn register_external_cpu(
        &mut self,
        identity: &PeripheralHandle,
        cpu: Box<dyn ExternalCpu>,
    ) -> Result<usize, &'static str> {
        if cpu.architecture().is_empty() {
            return Err("external CPU requires an architecture");
        }
        if self.external_cpu_slot(identity).is_some() {
            return Err("external CPU already registered");
        }
        let slot = self.external_cpus.len();
        self.external_cpus.push(Registration {
            identity: identity.clone(),
            cpu,
        });
        Ok(slot)
    }

    pub fn external_cpu_slot(&self, identity: &PeripheralHandle) -> Option<usize> {
        self.external_cpus
            .iter()
            .position(|entry| entry.identity == *identity)
    }

    /// Runs registered external CPUs concurrently while dispatching their reads
    /// on the machine thread. Each read completes before the next is dispatched.
    pub fn run_external_cpus_for(&mut self, duration: Duration) -> io::Result<()> {
        let deadline = Instant::now().checked_add(duration).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "external CPU run duration overflow",
            )
        })?;
        let (requests, incoming) = mpsc::channel::<ReadRequest>();
        let running = Arc::new(AtomicBool::new(true));
        let bus = &mut self.bus;
        bus.start_registered_peripherals();
        let result = thread::scope(|scope| {
            let threads: Vec<_> = self
                .external_cpus
                .iter_mut()
                .map(|entry| {
                    let channel = CpuBus {
                        requests: requests.clone(),
                        running: running.clone(),
                    };
                    scope.spawn(move || entry.cpu.run(&channel))
                })
                .collect();
            drop(requests);
            while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                match incoming.recv_timeout(remaining) {
                    Ok(request) => {
                        let result = bus
                            .read_unit(request.address, Width::DoubleWord)
                            .map(|value| value as u32);
                        let _ = request.reply.send(result);
                    }
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
                }
            }
            running.store(false, Ordering::Release);
            // Dropping pending reply senders unblocks reads submitted as the run ended.
            drop(incoming);
            let mut failed = false;
            for worker in threads {
                failed |= worker.join().is_err();
            }
            if failed {
                Err(io::Error::other("external CPU worker panicked"))
            } else {
                Ok(())
            }
        });
        bus.pause_registered_peripherals();
        result
    }
}
