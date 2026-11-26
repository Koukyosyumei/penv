use crate::machine::Machine;

#[test]
fn read_write_memory_bigger_than_2gb() {
    const SIZE: u32 = 3 * 1024 * 1024 * 1024;
    const START: u32 = 100 * 1024 * 1024;
    let mut machine = Machine::without_default_ram();
    machine.bus.map_ram(START, SIZE as usize).unwrap();
    let first = START + 16;
    let last = START + SIZE - 16;
    let mut sink = std::io::sink();

    machine.bus.write(first, 1, 0x1, &mut sink).unwrap();
    machine.bus.write(last, 1, 0x2, &mut sink).unwrap();
    assert_eq!(machine.bus.read::<1>(first).unwrap(), 0x1);
    assert_eq!(machine.bus.read::<1>(last).unwrap(), 0x2);
}

#[test]
fn should_return_one_pointer_when_segment_is_touched_concurrently() {
    use crate::mapped_memory::MappedMemory;
    use std::collections::HashSet;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicUsize, Ordering},
    };

    const THREAD_COUNT: usize = 64;
    const ATTEMPT_COUNT: usize = 20;
    for _ in 0..ATTEMPT_COUNT {
        let touched = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&touched);
        let memory = MappedMemory::new(64 * 1024, 64 * 1024, move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
        let mut segments = memory.mapped_segments();
        assert_eq!(segments.len(), 1);
        let segment = segments.next().unwrap();
        let barrier = Barrier::new(THREAD_COUNT + 1);
        let pointers = std::thread::scope(|scope| {
            let threads: Vec<_> = (0..THREAD_COUNT)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        segment.touch();
                        segment.pointer().unwrap().addr()
                    })
                })
                .collect();
            barrier.wait();
            threads
                .into_iter()
                .map(|thread| thread.join().unwrap())
                .collect::<HashSet<_>>()
        });
        assert_eq!(touched.load(Ordering::Relaxed), 1);
        assert_eq!(pointers.len(), 1);
    }
}
