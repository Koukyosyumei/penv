use crate::pseudorandom::Pseudorandom;
use std::{
    sync::{Arc, mpsc},
    thread,
};

const SEED: i32 = 0xC0DE;

struct Worker {
    sender: mpsc::Sender<(Arc<Pseudorandom>, mpsc::Sender<i32>)>,
    thread: thread::JoinHandle<()>,
}

impl Worker {
    fn new(name: &str) -> Self {
        let (sender, receiver) = mpsc::channel::<(Arc<Pseudorandom>, mpsc::Sender<i32>)>();
        let thread = thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                for (generator, reply) in receiver {
                    reply.send(generator.next().unwrap()).unwrap();
                }
            })
            .unwrap();
        Self { sender, thread }
    }

    fn next(&self, generator: &Arc<Pseudorandom>) -> i32 {
        let (reply, received) = mpsc::channel();
        self.sender.send((Arc::clone(generator), reply)).unwrap();
        received.recv().unwrap()
    }

    fn finish(self) {
        drop(self.sender);
        self.thread.join().unwrap();
    }
}

struct Setup {
    generator: Arc<Pseudorandom>,
    workers: [Worker; 2],
    expected: [Vec<i32>; 2],
}

impl Setup {
    fn new() -> Self {
        let workers = [Worker::new("Thread1"), Worker::new("Thread2")];
        let generator = Arc::new(Pseudorandom::new(SEED));
        generator.reset_seed(SEED);
        let expected = workers
            .each_ref()
            .map(|worker| (0..10).map(|_| worker.next(&generator)).collect());
        Self {
            generator,
            workers,
            expected,
        }
    }
}

#[test]
fn should_generate_numbers_deterministically() {
    let Setup {
        generator,
        workers,
        expected,
    } = Setup::new();
    generator.reset_seed(SEED);
    let actual = workers
        .each_ref()
        .map(|worker| (0..10).map(|_| worker.next(&generator)).collect::<Vec<_>>());
    assert_eq!(actual, expected);
    workers.into_iter().for_each(Worker::finish);
}

#[test]
fn should_generate_numbers_deterministically_multithreaded() {
    let Setup {
        generator,
        workers,
        expected,
    } = Setup::new();
    generator.reset_seed(SEED);
    let mut results = [Vec::new(), Vec::new()];
    for _ in 0..10 {
        for (worker, result) in workers.iter().zip(&mut results) {
            result.push(worker.next(&generator));
        }
    }
    assert_eq!(results, expected);
    workers.into_iter().for_each(Worker::finish);
}

#[test]
fn should_handle_serialization() {
    let Setup {
        generator, workers, ..
    } = Setup::new();
    let mut stream = Vec::new();
    generator.save(&mut stream).unwrap();
    let clone = Arc::new(Pseudorandom::load(stream.as_slice()).unwrap());
    assert_eq!(workers[0].next(&generator), workers[0].next(&clone));
    workers.into_iter().for_each(Worker::finish);
}
