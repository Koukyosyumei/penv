use crate::time_source::{
    TimeSource,
    paced::{MasterTimeSource, SlaveTimeSource},
};
use std::time::{Duration, Instant};

#[test]
fn should_not_sleep_on_advance_immediately() {
    const SLAVES: usize = 5;
    const ROUNDS: u64 = 3;
    const QUANTUM: u64 = 1_000_000_000;
    let mut master = MasterTimeSource::<QUANTUM>::new(true);
    let mut sinks = std::array::from_fn::<_, SLAVES, _>(|_| TimeSource::<QUANTUM>::default());
    let stopwatch = Instant::now();

    master
        .run(ROUNDS + 1, |boundary| {
            for sink in &mut sinks {
                sink.grant_until(boundary);
            }
            Ok(())
        })
        .unwrap();

    assert!(stopwatch.elapsed() < Duration::from_secs(ROUNDS));
}

#[test]
fn should_not_tick_disconnected_slave_time_source() {
    let mut master = MasterTimeSource::<10>::new(true);
    let mut slave = SlaveTimeSource::<10>::default();
    let mut sink = TimeSource::<10>::default();

    master
        .run(2, |boundary| {
            slave.advance_to(boundary, |end| sink.grant_until(end))
        })
        .unwrap();
    assert_eq!(master.sync_points(), 2);
    assert_eq!(slave.sync_points(), 2);
    assert_eq!(sink.sync_points().saturating_sub(1), 1);
    assert_eq!(master.elapsed(), 10);
    assert_eq!(slave.elapsed(), 10);
    assert_eq!(sink.elapsed(), 10);

    slave.disconnect();
    master
        .run(1, |boundary| {
            slave.advance_to(boundary, |end| sink.grant_until(end))
        })
        .unwrap();
    assert_eq!(master.sync_points(), 3);
    assert_eq!(slave.sync_points(), 2);
    assert_eq!(sink.sync_points().saturating_sub(1), 1);
    assert_eq!(master.elapsed(), 20);
    assert_eq!(slave.elapsed(), 10);
    assert_eq!(sink.elapsed(), 10);
}

#[test]
fn should_record_and_play_events() {
    use crate::{
        bus::RAM_BASE,
        bus_translation::{NativePeripheral, PeripheralHandle, Port},
        dma_engine::Width,
        event_trace::ExternalEvent,
        machine::Machine,
        pseudorandom::Pseudorandom,
    };
    use std::{
        cell::RefCell,
        path::PathBuf,
        rc::Rc,
        sync::atomic::{AtomicU64, Ordering},
    };

    type Events = Rc<RefCell<Vec<(u64, i32)>>>;
    struct PeripheralMock(Events);
    impl NativePeripheral for PeripheralMock {
        fn read(&mut self, _: Option<&str>, _: u64) -> u64 {
            0
        }
        fn write(&mut self, _: Option<&str>, _: u64, _: u64) {}
        fn external_call(&mut self, time: u64, method: &str, args: &[i32]) -> Result<(), String> {
            match (method, args) {
                ("Method", [counter]) | ("MethodTwoArgs", [counter, 0]) => {
                    self.0.borrow_mut().push((time, *counter));
                    Ok(())
                }
                _ => Err(format!("unexpected test peripheral call {method:?}")),
            }
        }
    }

    fn machine_factory() -> (Machine, Events) {
        let mut machine = Machine::without_default_ram();
        let events = Events::default();
        let handle = PeripheralHandle::default();
        machine
            .bus
            .register_translated_peripheral(
                &handle,
                vec![Port {
                    base: 0,
                    length: 1,
                    region: None,
                }],
                Width::Byte,
                Box::new(PeripheralMock(Rc::clone(&events))),
            )
            .unwrap();
        machine
            .bus
            .name_translated_peripheral(&handle, "mock")
            .unwrap();
        // Rust's virtual clock advances with the dispatcher. A self-jump keeps
        // the CPU from touching the mock while time is advanced.
        machine.bus.map_ram(RAM_BASE, 4096).unwrap();
        machine
            .bus
            .write(RAM_BASE, 4, 0x0000_006f, &mut std::io::sink())
            .unwrap();
        (machine, events)
    }

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
    struct TraceFile(PathBuf);
    impl Drop for TraceFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let trace = TraceFile(std::env::temp_dir().join(format!(
        "penv-nunit-time-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    )));
    let (mut machine, recorded) = machine_factory();
    machine.resume_cpu();
    machine.record_to(&trace.0).unwrap();
    let random = Pseudorandom::new(0xc0de);
    for i in 0..100 {
        for (method, arguments) in [("Method", vec![i]), ("MethodTwoArgs", vec![i, 0])] {
            machine
                .external_event(ExternalEvent::PeripheralCall {
                    peripheral: "sysbus.mock".into(),
                    method: method.into(),
                    arguments,
                })
                .unwrap();
        }
        let delay = (i64::from(random.next().unwrap()) * 30 / i64::from(i32::MAX)) as u64;
        std::thread::sleep(Duration::from_millis(delay));
        machine.run(delay, &mut std::io::sink()).unwrap();
    }
    machine.pause_cpu();
    let end = machine.elapsed_steps();
    drop(machine);

    let (mut machine, played) = machine_factory();
    machine.play_from(&trace.0).unwrap();
    machine.resume_cpu();
    while machine.elapsed_steps() < end {
        machine.run(1, &mut std::io::sink()).unwrap();
    }
    machine.pause_cpu();
    assert_eq!(*recorded.borrow(), *played.borrow());
}
