use crate::migrant::primitive::{Reader, Writer};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::sync::Mutex;

const MODULUS: i32 = i32::MAX;
const SLOTS: usize = 55;

struct Sequence {
    samples: [i32; SLOTS],
    next: usize,
}

impl Sequence {
    fn new(seed: i32) -> Self {
        let mut samples = [0; SLOTS];
        let mut previous = 161_803_398 - seed.saturating_abs();
        let mut next = 1;
        samples[SLOTS - 1] = previous;
        for i in 1..SLOTS {
            let index = (21 * i) % SLOTS - 1;
            samples[index] = next;
            next = previous.wrapping_sub(next);
            if next < 0 {
                next += MODULUS;
            }
            previous = samples[index];
        }
        for _ in 0..4 {
            for index in 0..SLOTS {
                let value = samples[index].wrapping_sub(samples[(index + 31) % SLOTS]);
                samples[index] = if value < 0 { value + MODULUS } else { value };
            }
        }
        Self { samples, next: 0 }
    }

    fn next(&mut self) -> i32 {
        let mut value = self.samples[self.next] - self.samples[(self.next + 21) % SLOTS];
        if value == MODULUS {
            value -= 1;
        }
        if value < 0 {
            value += MODULUS;
        }
        self.samples[self.next] = value;
        self.next = (self.next + 1) % SLOTS;
        value
    }
}

struct State {
    seed: i32,
    per_thread: BTreeMap<String, Sequence>,
}

/// Seeded .NET-compatible random streams, keyed by the UTF-8 name of each caller.
pub struct Pseudorandom {
    state: Mutex<State>,
}

impl Pseudorandom {
    pub fn new(seed: i32) -> Self {
        Self {
            state: Mutex::new(State {
                seed,
                per_thread: BTreeMap::new(),
            }),
        }
    }

    pub fn reset_seed(&self, seed: i32) {
        let mut state = self.state.lock().unwrap();
        state.seed = seed;
        state.per_thread.clear();
    }

    pub fn next(&self) -> io::Result<i32> {
        let thread = std::thread::current();
        let name = thread
            .name()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "random stream requires a named thread",
                )
            })?;
        let mut state = self.state.lock().unwrap();
        let seed = name
            .bytes()
            .fold(0i32, |sum, byte| sum.wrapping_add(i32::from(byte)))
            ^ state.seed;
        Ok(state
            .per_thread
            .entry(name.into())
            .or_insert_with(|| Sequence::new(seed))
            .next())
    }

    pub fn save(&self, stream: impl Write) -> io::Result<()> {
        let state = self.state.lock().unwrap();
        let mut writer = Writer::new(stream, false);
        writer.signed(i64::from(state.seed))?;
        writer.unsigned(state.per_thread.len() as u64)?;
        for (name, sequence) in &state.per_thread {
            writer.string(name)?;
            writer.unsigned(sequence.next as u64)?;
            for sample in sequence.samples {
                writer.signed(i64::from(sample))?;
            }
        }
        writer.finish()
    }

    pub fn load(stream: impl Read) -> io::Result<Self> {
        fn invalid() -> io::Error {
            io::Error::new(io::ErrorKind::InvalidData, "invalid pseudorandom state")
        }
        let mut reader = Reader::new(stream, false);
        let seed = i32::try_from(reader.signed()?).map_err(|_| invalid())?;
        let count = reader.unsigned()?;
        let mut per_thread = BTreeMap::new();
        for _ in 0..count {
            let name = reader.string()?;
            let next = reader.unsigned()?;
            if name.is_empty() || next >= SLOTS as u64 {
                return Err(invalid());
            }
            let mut samples = [0; SLOTS];
            for sample in &mut samples {
                *sample = i32::try_from(reader.signed()?).map_err(|_| invalid())?;
                if !(0..MODULUS).contains(sample) {
                    return Err(invalid());
                }
            }
            if per_thread
                .insert(
                    name,
                    Sequence {
                        samples,
                        next: next as usize,
                    },
                )
                .is_some()
            {
                return Err(invalid());
            }
        }
        reader.finish()?;
        Ok(Self {
            state: Mutex::new(State { seed, per_thread }),
        })
    }
}
