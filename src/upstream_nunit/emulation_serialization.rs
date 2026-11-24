use std::io::{Cursor, Seek, SeekFrom};

use crate::mapped_memory::MappedMemory;

use super::migrant::Random;

#[test]
fn serialize_memory() {
    const SIZE: usize = 128 * 1024 * 1024;
    const COUNT: usize = 30;
    const LENGTH: usize = 100 * 1024;

    let mut rng = Random(0x172b_f045);
    let mut buffers: Vec<(usize, Vec<u8>)> = Vec::with_capacity(COUNT);
    for _ in 0..COUNT {
        let buffer = (0..LENGTH).map(|_| rng.next(256) as u8).collect();
        let address = loop {
            let address = rng.next((SIZE - LENGTH) as i32) as usize;
            if buffers
                .iter()
                .all(|(previous, _)| previous.abs_diff(address) > LENGTH)
            {
                break address;
            }
        };
        buffers.push((address, buffer));
    }

    let mut stream = Cursor::new(Vec::new());
    {
        // Upstream chooses a segment size of size / 16 for this memory.
        let mut memory = MappedMemory::new(SIZE, SIZE / 16, |_| {}).unwrap();
        for (address, buffer) in &buffers {
            memory.write_bytes(*address, buffer).unwrap();
        }
        memory.save(&mut stream).unwrap();
    }

    stream.seek(SeekFrom::Start(0)).unwrap();
    let memory = MappedMemory::load(stream).unwrap();
    for (address, buffer) in buffers {
        let mut copy = vec![0; LENGTH];
        memory.read_bytes(address, &mut copy).unwrap();
        assert_eq!(copy, buffer, "buffer at {address:#x}");
    }
}

use crate::{
    migrant::object_stream::{
        Graph, Object, ObjectId, ObjectReader, ObjectWriter, References, Value,
    },
    nrf52840::gpio_line::{Endpoint, GpioRoutes},
};

struct GpioMocks {
    routes: GpioRoutes<u32>,
    devices: Vec<GpioMock>,
}

struct GpioMock {
    id: u32,
    outputs: [bool; 10],
    active_inputs: std::collections::BTreeSet<u32>,
}

impl GpioMock {
    fn signal(&mut self, input: u32, high: bool) {
        if high {
            self.active_inputs.insert(input);
        } else {
            self.active_inputs.remove(&input);
        }
    }
}

impl GpioMocks {
    fn new(ids: impl IntoIterator<Item = u32>) -> Self {
        Self {
            routes: GpioRoutes::default(),
            devices: ids
                .into_iter()
                .map(|id| GpioMock {
                    id,
                    outputs: [false; 10],
                    active_inputs: Default::default(),
                })
                .collect(),
        }
    }

    fn device_mut(&mut self, id: u32) -> &mut GpioMock {
        self.devices
            .iter_mut()
            .find(|device| device.id == id)
            .unwrap()
    }

    fn connect(&mut self, source: u32, pin: u8, receiver: u32, input: u32) {
        let high = self.device_mut(source).outputs[usize::from(pin)];
        let line = self.routes.line_mut(source, pin);
        let count = line.endpoints().len();
        line.connect(receiver, "GPIOMock", 0, input).unwrap();
        if line.endpoints().len() != count {
            self.device_mut(receiver).signal(input, high);
        }
    }

    fn set(&mut self, source: u32, pin: u8, high: bool) {
        let output = &mut self.device_mut(source).outputs[usize::from(pin)];
        if *output == high {
            return;
        }
        *output = high;
        for (output, line) in self.routes.lines(source) {
            if output != pin {
                continue;
            }
            for endpoint in line.endpoints() {
                let receiver = self
                    .devices
                    .iter_mut()
                    .find(|device| device.id == endpoint.receiver)
                    .unwrap();
                receiver.signal(endpoint.input, high);
            }
        }
    }

    fn endpoints(&self, id: u32, pin: u8) -> &[Endpoint<u32>] {
        self.routes
            .lines(id)
            .find(|(output, _)| *output == pin)
            .map_or(&[], |(_, line)| line.endpoints())
    }

    fn deep_clone(&self) -> Self {
        let mut graph = Graph::default();
        let ids: std::collections::BTreeMap<_, _> = self
            .devices
            .iter()
            .map(|device| (device.id, graph.insert(Object::Array(Vec::new()))))
            .collect();
        for device in &self.devices {
            let inputs = graph.insert(Object::Array(
                device
                    .active_inputs
                    .iter()
                    .map(|&input| Value::Integer(input.into()))
                    .collect(),
            ));
            let outputs: Vec<_> = device
                .outputs
                .iter()
                .enumerate()
                .map(|(pin, &high)| {
                    let mut fields = vec![Value::Boolean(high)];
                    for endpoint in self.endpoints(device.id, pin as u8) {
                        fields.push(Value::Integer(endpoint.input.into()));
                        fields.push(Value::Ref(ids[&endpoint.receiver]));
                    }
                    Value::Ref(graph.insert(Object::Array(fields)))
                })
                .collect();
            let outputs = graph.insert(Object::Array(outputs));
            *graph.get_mut(ids[&device.id]).unwrap() = Object::Array(vec![
                Value::Integer(device.id.into()),
                Value::Ref(inputs),
                Value::Ref(outputs),
            ]);
        }
        let root = graph.insert(Object::Array(
            self.devices
                .iter()
                .map(|device| Value::Ref(ids[&device.id]))
                .collect(),
        ));
        let mut stream = Cursor::new(Vec::new());
        let mut writer = ObjectWriter::new(&mut stream, &graph, References::Preserve, true);
        writer.write_object(root).unwrap();
        writer.finish().unwrap();
        stream.seek(SeekFrom::Start(0)).unwrap();
        let mut reader = ObjectReader::new(stream, References::Preserve, true);
        let root = reader.read_object().unwrap();

        let array = |id: ObjectId| {
            let Object::Array(fields) = reader.graph().get(id).unwrap() else {
                panic!("expected array")
            };
            fields.as_slice()
        };
        let reference = |value: &Value| {
            let Value::Ref(id) = value else {
                panic!("expected reference")
            };
            *id
        };
        let number = |value: &Value| {
            let Value::Integer(number) = value else {
                panic!("expected integer")
            };
            u32::try_from(*number).unwrap()
        };
        let device_ids: std::collections::HashMap<_, _> = array(root)
            .iter()
            .map(|device| reference(device))
            .map(|device| (device, number(&array(device)[0])))
            .collect();
        let mut restored = Self::new(
            array(root)
                .iter()
                .map(|device| device_ids[&reference(device)]),
        );
        for device in array(root) {
            let record = array(reference(device));
            let id = number(&record[0]);
            let mock = restored.device_mut(id);
            mock.active_inputs
                .extend(array(reference(&record[1])).iter().map(number));
            for (pin, output) in array(reference(&record[2])).iter().enumerate() {
                let fields = array(reference(output));
                restored.device_mut(id).outputs[pin] = fields[0] == Value::Boolean(true);
                for pair in fields[1..].chunks_exact(2) {
                    let receiver = device_ids[&reference(&pair[1])];
                    restored
                        .routes
                        .line_mut(id, pin as u8)
                        .connect(receiver, "GPIOMock", 0, number(&pair[0]))
                        .unwrap();
                }
            }
        }
        restored
    }
}

#[test]
fn serialize_gpios() {
    let mut mocks = GpioMocks::new([1, 2, 3, 4]);
    mocks.connect(1, 0, 2, 5);
    mocks.connect(1, 0, 4, 7);
    mocks.connect(2, 1, 3, 0);
    mocks.connect(3, 2, 2, 6);
    mocks.set(1, 0, true);
    mocks.set(3, 1, true);
    mocks.set(3, 2, true);

    let mut cloned = mocks.deep_clone();
    assert_eq!(cloned.devices.len(), 4);
    assert_eq!(
        cloned.endpoints(1, 0)[0],
        Endpoint {
            input: 5,
            receiver: 2
        }
    );
    assert_eq!(
        cloned.endpoints(1, 0)[1],
        Endpoint {
            input: 7,
            receiver: 4
        }
    );
    assert_eq!(
        cloned.endpoints(2, 1)[0],
        Endpoint {
            input: 0,
            receiver: 3
        }
    );
    assert_eq!(
        cloned.endpoints(3, 2)[0],
        Endpoint {
            input: 6,
            receiver: 2
        }
    );

    assert!(
        cloned.device_mut(2).active_inputs.contains(&5),
        "Mock 2:5 is not signaled!"
    );
    assert!(
        cloned.device_mut(2).active_inputs.contains(&6),
        "Mock 2:6 is not signaled!"
    );
    assert!(
        !cloned.device_mut(3).active_inputs.contains(&0),
        "Mock 3:0 is signaled!"
    );
    cloned.connect(3, 1, 1, 9);
    assert!(
        cloned.device_mut(1).active_inputs.contains(&9),
        "Mock 1:9 is not signaled!"
    );
}

use crate::{bus::CpuRegistrationPoint, bus_translation::PeripheralHandle, machine::Machine};

struct MockCpu {
    handle: PeripheralHandle,
    placeholder: Option<String>,
}

fn clone_machine_with_mock_cpus(placeholders: &[Option<&str>]) -> (Machine, Vec<MockCpu>) {
    let mut machine = Machine::without_default_ram();
    let cpus: Vec<_> = placeholders
        .iter()
        .map(|&placeholder| {
            let cpu = MockCpu {
                handle: PeripheralHandle::default(),
                placeholder: placeholder.map(str::to_owned),
            };
            machine
                .bus
                .register_cpu_identity(&cpu.handle, "mock", Some(CpuRegistrationPoint::default()))
                .unwrap();
            if let Some(name) = placeholder {
                machine.bus.set_cpu_local_name(&cpu.handle, name).unwrap();
            }
            cpu
        })
        .collect();

    let mut stream = Cursor::new(Vec::new());
    machine
        .bus
        .save_cpu_registrations(&mut stream, |handle| {
            Ok(cpus
                .iter()
                .find(|cpu| &cpu.handle == handle)
                .unwrap()
                .placeholder
                .clone())
        })
        .unwrap();
    stream.set_position(0);
    let mut copy = Machine::without_default_ram();
    let cpus = copy
        .bus
        .restore_cpu_registrations(stream)
        .unwrap()
        .into_iter()
        .map(|(handle, placeholder)| MockCpu {
            handle,
            placeholder,
        })
        .collect();
    (copy, cpus)
}

fn assert_cpu_count(count: usize) {
    let (copy, _) = clone_machine_with_mock_cpus(&vec![None; count]);
    assert_eq!(copy.bus.cpu_registrations().len(), count);
}

#[test]
fn serialize_one_cpu() {
    assert_cpu_count(1);
}

#[test]
fn serialize_two_cpus() {
    assert_cpu_count(2);
}

#[test]
fn serialize_three_cpus() {
    assert_cpu_count(3);
}

#[test]
fn serialize_four_cpus() {
    assert_cpu_count(4);
}

#[test]
fn remember_cpu_names() {
    let names = ["Xavier", "Alice", "Bob"];
    let (machine, cpus) = clone_machine_with_mock_cpus(&names.map(Some));
    let registered = machine.bus.cpu_registrations();
    let mut restored_names: Vec<_> = registered
        .iter()
        .map(|cpu| cpu.local_name.as_deref().unwrap())
        .collect();
    restored_names.sort_unstable();
    let mut expected = names;
    expected.sort_unstable();
    assert_eq!(restored_names, expected);
    for cpu in cpus {
        let local_name = &registered
            .iter()
            .find(|entry| entry.handle == cpu.handle)
            .unwrap()
            .local_name;
        assert_eq!(&cpu.placeholder, local_name);
    }
}
