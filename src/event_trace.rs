use std::{
    collections::VecDeque,
    fs::File,
    io::{BufRead, BufReader, Write},
    path::Path,
};

const HEADER: &str = "penv-external-events-v1";

#[derive(Debug, PartialEq)]
pub enum ExternalEvent {
    Button(String, bool),
    Adc(String, usize, f64),
    UartLine(String, String, Vec<u8>),
    PeripheralCall {
        peripheral: String,
        method: String,
        arguments: Vec<i32>,
    },
}

pub struct Recorder(File);

impl Recorder {
    pub fn create(path: &Path) -> Result<Self, String> {
        let mut file = File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
        writeln!(file, "{HEADER}").map_err(|e| e.to_string())?;
        Ok(Self(file))
    }

    pub fn record(&mut self, step: u64, event: &ExternalEvent) -> Result<(), String> {
        let result = match event {
            ExternalEvent::Button(name, pressed) => writeln!(
                self.0,
                "{step}\tbutton\t{}\t{pressed}",
                encode(name.as_bytes())
            ),
            ExternalEvent::Adc(name, channel, volts) => writeln!(
                self.0,
                "{step}\tadc\t{}\t{channel}\t{volts}",
                encode(name.as_bytes())
            ),
            ExternalEvent::PeripheralCall {
                peripheral,
                method,
                arguments,
            } => {
                let arguments = arguments
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                writeln!(
                    self.0,
                    "{step}\tcall\t{}\t{}\t{arguments}",
                    encode(peripheral.as_bytes()),
                    encode(method.as_bytes())
                )
            }
            ExternalEvent::UartLine(name, text, ending) => writeln!(
                self.0,
                "{step}\tuart\t{}\t{}\t{}",
                encode(name.as_bytes()),
                encode(text.as_bytes()),
                encode(ending)
            ),
        };
        result.map_err(|e| e.to_string())
    }
}

pub fn read(path: &Path) -> Result<VecDeque<(u64, ExternalEvent)>, String> {
    let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut lines = BufReader::new(file).lines();
    if lines
        .next()
        .transpose()
        .map_err(|e| e.to_string())?
        .as_deref()
        != Some(HEADER)
    {
        return Err(format!(
            "{}: invalid external event trace header",
            path.display()
        ));
    }
    let mut trace = VecDeque::new();
    for (index, line) in lines.enumerate() {
        let error = |e: &str| format!("{}:{}: {e}", path.display(), index + 2);
        let line = line.map_err(|e| error(&e.to_string()))?;
        let (step, event) = parse(&line).map_err(error)?;
        if trace.back().is_some_and(|(previous, _)| *previous > step) {
            return Err(error("event timestamps must be nondecreasing"));
        }
        trace.push_back((step, event));
    }
    Ok(trace)
}

fn parse(line: &str) -> Result<(u64, ExternalEvent), &'static str> {
    let (step, fields) = line.split_once('\t').ok_or("missing event fields")?;
    let step = step.parse().map_err(|_| "invalid event timestamp")?;
    let fields: Vec<_> = fields.split('\t').collect();
    let event = match fields.as_slice() {
        ["button", name, pressed] => ExternalEvent::Button(
            decode_string(name)?,
            pressed.parse().map_err(|_| "invalid button state")?,
        ),
        ["adc", name, channel, volts] => ExternalEvent::Adc(
            decode_string(name)?,
            channel.parse().map_err(|_| "invalid ADC channel")?,
            volts.parse().map_err(|_| "invalid ADC voltage")?,
        ),
        ["call", peripheral, method, args] => ExternalEvent::PeripheralCall {
            peripheral: decode_string(peripheral)?,
            method: decode_string(method)?,
            arguments: if args.is_empty() {
                Vec::new()
            } else {
                args.split(',')
                    .map(|arg| arg.parse().map_err(|_| "invalid peripheral argument"))
                    .collect::<Result<_, _>>()?
            },
        },
        ["uart", name, text, ending] => {
            ExternalEvent::UartLine(decode_string(name)?, decode_string(text)?, decode(ending)?)
        }
        _ => return Err("unknown external event"),
    };
    Ok((step, event))
}

fn encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode(value: &str) -> Result<Vec<u8>, &'static str> {
    crate::hex_string::decode(value, false, false).map_err(|_| "invalid event hex string")
}

fn decode_string(value: &str) -> Result<String, &'static str> {
    String::from_utf8(decode(value)?).map_err(|_| "invalid event UTF-8")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renode_time_source_records_and_replays_external_events_in_order() {
        let path = std::env::temp_dir().join(format!("penv-event-trace-{}", std::process::id()));
        let events = (0..100)
            .flat_map(|i| {
                [
                    (i * 13, ExternalEvent::Button("button".into(), i % 2 == 0)),
                    (
                        i * 13,
                        ExternalEvent::Adc("adc".into(), 0, i as f64 / 100.0),
                    ),
                ]
            })
            .chain([(
                1300,
                ExternalEvent::UartLine("uart".into(), "µ\t".into(), b"\r\n".to_vec()),
            )])
            .collect::<Vec<_>>();
        let mut recorder = Recorder::create(&path).unwrap();
        for (step, event) in &events {
            recorder.record(*step, event).unwrap();
        }
        drop(recorder);
        assert_eq!(read(&path).unwrap(), events);
        std::fs::remove_file(path).unwrap();
    }
}
