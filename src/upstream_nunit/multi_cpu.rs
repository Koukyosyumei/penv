use std::{
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use crate::{
    bus_translation::{NativePeripheral, PeripheralHandle, Port},
    dma_engine::Width,
    machine::{
        external_cpu::{CpuBus, ExternalCpu},
        Machine,
    },
    pseudorandom::Sequence,
};

struct MockCpu;

impl ExternalCpu for MockCpu {
    fn architecture(&self) -> &str {
        "mock"
    }
    fn run(&mut self, _: &CpuBus) {}
}

#[test]
fn should_enumerate_cpus() {
    let mut machine = Machine::without_default_ram();
    let cpus: [PeripheralHandle; 10] = std::array::from_fn(|_| PeripheralHandle::default());
    for cpu in &cpus {
        machine
            .register_external_cpu(cpu, Box::new(MockCpu))
            .unwrap();
    }
    for (index, cpu) in cpus.iter().enumerate() {
        assert_eq!(machine.external_cpu_slot(cpu), Some(index));
    }
}

struct ActivelyAskingCpu {
    address: u64,
    reads: Arc<AtomicU64>,
}
impl ExternalCpu for ActivelyAskingCpu {
    fn architecture(&self) -> &str {
        "mock"
    }

    fn run(&mut self, bus: &CpuBus) {
        while let Some(value) = bus.read_doubleword(self.address) {
            value.unwrap();
            self.reads.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct ActivelyAskedPeripheral {
    counter: AtomicI64,
    failed: Arc<AtomicBool>,
    random: Sequence,
}
impl NativePeripheral for ActivelyAskedPeripheral {
    fn read(&mut self, _: Option<&str>, _: u64) -> u64 {
        let value = self.counter.load(Ordering::SeqCst);
        let wait = self.random.next() % 10_000;
        for _ in 0..wait {
            std::hint::spin_loop();
        }
        let exchanged = self.counter.swap(value + 1, Ordering::SeqCst);
        if exchanged != value {
            self.failed.store(true, Ordering::SeqCst);
        }
        wait as u64
    }

    fn write(&mut self, _: Option<&str>, _: u64, _: u64) {}
}

fn guard_peripheral_reads(cpu_count: usize) {
    let mut machine = Machine::without_default_ram();
    let mut reads = Vec::new();
    for _ in 0..cpu_count {
        let counter = Arc::new(AtomicU64::new(0));
        machine
            .register_external_cpu(
                &PeripheralHandle::default(),
                Box::new(ActivelyAskingCpu {
                    address: 0,
                    reads: counter.clone(),
                }),
            )
            .unwrap();
        reads.push(counter);
    }
    let failed = Arc::new(AtomicBool::new(false));
    machine
        .bus
        .map_translated_peripheral(
            vec![Port {
                base: 0,
                length: 1000,
                region: None,
            }],
            Width::DoubleWord,
            Box::new(ActivelyAskedPeripheral {
                counter: AtomicI64::new(0),
                failed: failed.clone(),
                random: Sequence::new(1),
            }),
        )
        .unwrap();
    machine
        .run_external_cpus_for(Duration::from_millis(1000))
        .unwrap();
    assert!(
        !failed.load(Ordering::SeqCst),
        "Peripheral was concurrently accessed from multiple CPUs."
    );
    assert!(reads.iter().all(|count| count.load(Ordering::Relaxed) > 0));
}

#[test]
fn should_guard_peripheral_reads_1() {
    guard_peripheral_reads(1);
}
#[test]
fn should_guard_peripheral_reads_2() {
    guard_peripheral_reads(2);
}
#[test]
fn should_guard_peripheral_reads_3() {
    guard_peripheral_reads(3);
}
#[test]
fn should_guard_peripheral_reads_4() {
    guard_peripheral_reads(4);
}
