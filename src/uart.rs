use std::{
    collections::VecDeque,
    io::{self, Write},
    sync::mpsc::{Receiver, TryRecvError},
};

pub(crate) struct MiVUart {
    pub(crate) base: u32,
    pub(crate) name: String,
    receive: VecDeque<u8>,
    control: [u8; 3],
}

impl MiVUart {
    pub(crate) const SIZE: u64 = 0x18;

    pub(crate) fn new(base: u32, name: String) -> Self {
        Self { base, name, receive: VecDeque::new(), control: [0; 3] }
    }

    pub(crate) fn contains(&self, address: u32) -> bool {
        address & 3 == 0 && u64::from(address.wrapping_sub(self.base)) < Self::SIZE
    }

    pub(crate) fn queue_input(&mut self, bytes: Vec<u8>) {
        self.receive.extend(bytes);
    }

    pub(crate) fn read(&mut self, address: u32) -> u32 {
        u32::from(match address - self.base {
            4 => self.receive.pop_front().unwrap_or(0),
            8 => self.control[0],
            12 => self.control[1],
            16 => 1 | (u8::from(!self.receive.is_empty()) << 1),
            20 => self.control[2],
            _ => 0,
        })
    }

    pub(crate) fn write(&mut self, address: u32, value: u32) -> Option<u8> {
        let byte = value as u8;
        match address - self.base {
            0 => return Some(byte),
            8 => self.control[0] = byte,
            12 => self.control[1] = byte,
            20 => self.control[2] = byte & 7,
            _ => {}
        }
        None
    }
}

pub struct MuraxUart {
    pub base: u32,
    irq_connected: bool,
    receive: VecDeque<u8>,
    receive_irq_enabled: bool,
    frame: u32,
}

impl MuraxUart {
    pub const SIZE: u64 = 16;

    pub fn new(base: u32, irq_connected: bool) -> Self {
        Self {
            base,
            irq_connected,
            receive: VecDeque::new(),
            receive_irq_enabled: false,
            frame: 0,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        u64::from(address.wrapping_sub(self.base)) < Self::SIZE
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        self.receive.extend(bytes);
    }

    fn data_ready(&self) -> bool {
        !self.receive.is_empty()
    }

    pub fn interrupt_pending(&self) -> bool {
        self.irq_connected && self.receive_irq_enabled && self.data_ready()
    }

    pub fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0 if self.data_ready() => u32::from(self.receive.pop_front().unwrap_or(0)),
            4 => (u32::from(self.data_ready()) << 9) | (u32::from(self.receive_irq_enabled) << 1),
            12 => self.frame,
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32, output: &mut impl Write) -> io::Result<()> {
        match address - self.base {
            0 => output.write_all(&[value as u8])?,
            4 => self.receive_irq_enabled = value & 2 != 0,
            12 => self.frame = value & 0x1_0300,
            _ => {}
        }
        Ok(())
    }
}

pub struct LiteXTxTiming {
    pub capacity: usize,
    pub flush_steps: u64,
    pub timeout_steps: u64,
}

impl Default for LiteXTxTiming {
    fn default() -> Self {
        Self {
            capacity: 8,
            flush_steps: 10,
            timeout_steps: 20_000_000,
        }
    }
}

struct LiteXTransmit {
    timing: LiteXTxTiming,
    fifo: Vec<u8>,
    deadline: u64,
    step: u64,
}

struct LiteXLiveInput {
    receiver: Receiver<io::Result<Vec<u8>>>,
    bytes: std::vec::IntoIter<u8>,
    next_step: u64,
}

pub struct LiteXUart {
    pub bases: Vec<u32>,
    pub vex_irq: Option<(usize, u8)>,
    receive: VecDeque<u8>,
    pending: u32,
    enabled: u32,
    irq: bool,
    legacy_console_events: bool,
    live_input: Option<LiteXLiveInput>,
    transmit: Option<LiteXTransmit>,
}

impl LiteXUart {
    pub const SIZE: u32 = 0x100;

    pub fn new(bases: Vec<u32>, vex_irq: Option<(usize, u8)>) -> Self {
        Self {
            bases,
            vex_irq,
            receive: VecDeque::new(),
            pending: 1,
            enabled: 0,
            irq: false,
            legacy_console_events: false,
            live_input: None,
            transmit: None,
        }
    }

    pub fn legacy_console_events(&mut self) {
        self.legacy_console_events = true;
    }

    pub fn with_timing(mut self, timing: Option<LiteXTxTiming>) -> Self {
        if let Some(timing) = timing.filter(|timing| timing.capacity > 0) {
            self.transmit = Some(LiteXTransmit {
                timing,
                fifo: Vec::new(),
                deadline: u64::MAX,
                step: 0,
            });
        }
        self
    }

    pub fn attach_live_input(&mut self, receiver: Receiver<io::Result<Vec<u8>>>) {
        self.live_input = Some(LiteXLiveInput {
            receiver,
            bytes: Vec::new().into_iter(),
            next_step: 0,
        });
    }

    fn tick_live_input(&mut self, step: u64) -> io::Result<()> {
        // LiteX reports 115200 baud. A byte occupies ten bits on the wire.
        const BYTE_STEPS: u64 = (100_000_000u64 * 10).div_ceil(115_200);
        const IDLE_POLL_STEPS: u64 = 1024;
        let Some(input) = self.live_input.as_mut() else {
            return Ok(());
        };
        if step < input.next_step {
            return Ok(());
        }
        if input.bytes.len() == 0 {
            match input.receiver.try_recv() {
                Ok(bytes) => input.bytes = bytes?.into_iter(),
                Err(TryRecvError::Empty) => {
                    input.next_step = step.saturating_add(IDLE_POLL_STEPS);
                    return Ok(());
                }
                Err(TryRecvError::Disconnected) => {
                    self.live_input = None;
                    return Ok(());
                }
            }
        }
        if let Some(byte) = input.bytes.next() {
            input.next_step = step.saturating_add(BYTE_STEPS);
            self.receive.push_back(byte);
            self.pending |= 2;
            self.update_irq();
        }
        Ok(())
    }

    pub fn tick(&mut self, step: u64, output: &mut impl Write) -> io::Result<()> {
        self.tick_live_input(step)?;
        let Some(tx) = self.transmit.as_mut() else {
            return Ok(());
        };
        tx.step = step;
        if step >= tx.deadline && !tx.fifo.is_empty() {
            output.write_all(&tx.fifo)?;
            tx.fifo.clear();
            tx.deadline = u64::MAX;
            self.pending |= 1;
            self.update_irq();
        }
        Ok(())
    }

    fn update_irq(&mut self) {
        self.irq = self.enabled & self.pending != 0;
    }

    pub fn offset(&self, address: u32, width: usize) -> Option<u32> {
        if !matches!(width, 1 | 4) {
            return None;
        }
        self.bases.iter().find_map(|base| {
            let offset = address.wrapping_sub(*base);
            (offset < Self::SIZE).then_some(offset)
        })
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.receive.extend(bytes);
            self.pending |= 2;
            self.update_irq();
        }
    }

    pub fn interrupt_pending(&self) -> bool {
        self.irq
    }

    pub fn read(&mut self, offset: u32) -> u32 {
        match offset {
            0 => {
                let byte = self.receive.pop_front().unwrap_or(0);
                if self.receive.is_empty() {
                    self.update_irq();
                }
                u32::from(byte)
            }
            4 => u32::from(
                self.transmit
                    .as_ref()
                    .is_some_and(|tx| tx.fifo.len() >= tx.timing.capacity),
            ),
            8 => u32::from(self.receive.is_empty()),
            // Legacy interrupt handlers may poll this until the IRQ deasserts.
            0x10 if self.legacy_console_events => self.pending & self.enabled,
            0x10 => self.pending,
            0x14 => self.enabled,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, output: &mut impl Write) -> io::Result<()> {
        match offset {
            0 if self.transmit.is_some() => {
                let tx = self.transmit.as_mut().unwrap();
                if tx.fifo.len() < tx.timing.capacity {
                    tx.fifo.push(value as u8);
                    let full = tx.fifo.len() >= tx.timing.capacity;
                    self.pending = (self.pending & !1) | u32::from(!full);
                    let delay = if full {
                        tx.timing.flush_steps
                    } else {
                        tx.timing.timeout_steps
                    };
                    tx.deadline = tx.step.saturating_add(delay);
                }
            }
            0 => {
                output.write_all(&[value as u8])?;
                self.pending |= 1;
                self.update_irq();
            }
            0x10 => {
                self.pending &= !value;
                // The legacy ISR acknowledges RX after reading a single byte.
                // Keep the remaining queued bytes visible to that handler.
                if self.legacy_console_events && !self.receive.is_empty() {
                    self.pending |= 2;
                }
                self.update_irq();
            }
            0x14 => {
                self.enabled = value & 3;
                self.update_irq();
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub enum UartWaitMode {
    Lf,
    Cr,
    Prompt,
}

struct UartLineMatch {
    end: u8,
    prompt: bool,
    pattern: Vec<u8>,
    regex: Option<regex::Regex>,
    tail: VecDeque<u8>,
    line: crate::circular_buffer::CircularBuffer<u8>,
    contains: bool,
    matched: bool,
    first_only: bool,
    completed_line: bool,
    matched_completed_line: bool,
}

impl UartLineMatch {
    fn new(pattern: &str, regex: Option<regex::Regex>, end: UartWaitMode, carriage_return: bool, first_only: bool) -> Self {
        Self {
            end: if carriage_return || matches!(end, UartWaitMode::Cr) { b'\r' } else { b'\n' },
            prompt: matches!(end, UartWaitMode::Prompt),
            pattern: pattern.as_bytes().to_vec(),
            regex,
            tail: VecDeque::new(),
            line: crate::circular_buffer::CircularBuffer::new(UART_HISTORY_BYTES),
            contains: false,
            matched: false,
            first_only,
            completed_line: false,
            matched_completed_line: false,
        }
    }

    fn receive(&mut self, byte: u8) {
        // TerminalTester text mode discards NUL and, for LF-delimited
        // lines, CR before matching. Raw output and other watchers keep them.
        if self.matched || byte == 0 || (self.end == b'\n' && byte == b'\r') {
            return;
        }
        if byte == self.end {
            self.matched = (!self.first_only || !self.completed_line)
                && (self.contains || self.regex_matches_line()
                    || (self.pattern.is_empty() && self.regex.is_none()));
            self.completed_line = true;
            self.matched_completed_line = self.matched;
            self.tail.clear();
            if !self.matched {
                self.line.clear();
            }
            self.contains = false;
        } else {
            self.line.enqueue(byte);
            if self.regex.is_some() {
                if self.prompt {
                    self.contains = self.regex_matches_line();
                }
            } else if !self.contains {
                if self.tail.len() == self.pattern.len() {
                    self.tail.pop_front();
                }
                self.tail.push_back(byte);
                self.contains = self.tail.iter().eq(self.pattern.iter());
            }
        }
    }

    fn prompt_matches(&self) -> bool {
        if !self.prompt {
            return false;
        }
        if !self.first_only {
            return self.contains;
        }
        if let Some(regex) = &self.regex {
            let bytes: Vec<_> = self.line.iter().copied().collect();
            return regex.find(&String::from_utf8_lossy(&bytes)).is_some_and(|m| m.start() == 0);
        }
        self.line.len() >= self.pattern.len()
            && self.line.iter().take(self.pattern.len()).eq(self.pattern.iter())
    }

    fn regex_matches_line(&self) -> bool {
        self.regex.as_ref().is_some_and(|pattern| {
            let bytes: Vec<_> = self.line.iter().copied().collect();
            pattern.is_match(&String::from_utf8_lossy(&bytes))
        })
    }
}

pub const UART_HISTORY_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct FailingUartPattern {
    pub pattern: String,
    pub regex: Option<regex::Regex>,
}

impl FailingUartPattern {
    fn matches(&self, line: &str) -> bool {
        self.regex.as_ref().map_or_else(
            || line.contains(&self.pattern),
            |regex| regex.is_match(line),
        )
    }
}

struct MultilineWait {
    patterns: Vec<FailingUartPattern>,
    candidates: Vec<usize>,
    first_only: bool,
    unfinished: bool,
    lines_seen: usize,
    line: Vec<u8>,
    matched: Option<(String, bool)>,
}

impl MultilineWait {
    fn finish_partial(&mut self) -> bool {
        if !self.unfinished || self.matched.is_some() || self.line.is_empty() {
            return false;
        }
        let last = self.patterns.len() - 1;
        let allowed = self.candidates.contains(&last) || last == 0 && (!self.first_only || self.lines_seen == 0);
        if !allowed {
            return false;
        }
        let line = String::from_utf8_lossy(&self.line);
        let pattern = &self.patterns[last];
        let matches = if self.first_only {
            pattern.regex.as_ref().map_or_else(
                || line.starts_with(&pattern.pattern),
                |regex| regex.find(&line).is_some_and(|found| found.start() == 0),
            )
        } else {
            pattern.matches(&line)
        };
        if matches {
            self.matched = Some((line.into_owned(), false));
            return true;
        }
        false
    }

    fn push(&mut self, byte: u8, end: u8) -> bool {
        if self.matched.is_some() || byte == 0 || (end == b'\n' && byte == b'\r') {
            return false;
        }
        if byte != end && self.line.len() != UART_HISTORY_BYTES {
            self.line.push(byte);
        }
        if byte != end {
            return self.finish_partial();
        }
        let last = self.patterns.len() - 1;
        let line = String::from_utf8_lossy(&self.line);
        if (self.candidates.contains(&last) || last == 0 && (!self.first_only || self.lines_seen == 0))
            && self.patterns[last].matches(&line)
        {
            self.matched = Some((line.into_owned(), true));
            return true;
        }
        self.candidates.retain_mut(|index| {
            if *index != last && self.patterns[*index].matches(&line) {
                *index += 1;
                true
            } else {
                false
            }
        });
        if last != 0 && (!self.first_only || self.lines_seen == 0) && self.patterns[0].matches(&line) {
            self.candidates.push(1);
        }
        self.lines_seen += 1;
        self.line.clear();
        false
    }
}

#[derive(Default)]
pub struct UartHistory {
    carriage_return: bool,
    unread: VecDeque<(u8, u128)>,
    matched_timestamp: Option<u128>,
    pub(crate) activity: u64,
    sgr: Vec<u8>,
    active: Option<UartLineMatch>,
    multiline: Option<MultilineWait>,
    failures: Vec<FailingUartPattern>,
    failure: Option<String>,
}

impl UartHistory {
    pub fn with_carriage_return(carriage_return: bool) -> Self {
        Self { carriage_return, ..Self::default() }
    }

    fn terminator(&self) -> u8 {
        if self.carriage_return { b'\r' } else { b'\n' }
    }

    pub(crate) fn clear_completed_lines(&mut self) {
        let terminator = self.terminator();
        if let Some(index) = self.unread.iter().rposition(|&(byte, _)| byte == terminator) {
            self.unread.drain(..=index);
        }
        self.matched_timestamp = None;
        self.failure = None;
    }

    pub fn set_failure(&mut self, pattern: FailingUartPattern, register: bool) {
        let index = self.failures.iter().position(|p| {
            p.pattern == pattern.pattern && p.regex.is_some() == pattern.regex.is_some()
        });
        match (register, index) {
            (true, None) => self.failures.push(pattern),
            (false, Some(index)) => {
                self.failures.remove(index);
            }
            _ => {}
        }
    }

    fn failing_line(&self) -> Option<(usize, String)> {
        if self.failures.is_empty() {
            return None;
        }
        let mut finished = Vec::new();
        let mut start = 0;
        for (index, &(byte, _)) in self.unread.iter().enumerate() {
            if byte == self.terminator() {
                finished.push((start, index));
                start = index + 1;
            }
        }
        for pattern in &self.failures {
            for &(start, end) in &finished {
                let line: Vec<_> = self
                    .unread
                    .iter()
                    .skip(start)
                    .take(end - start)
                    .map(|&(byte, _)| byte)
                    .filter(|&byte| byte != 0 && (self.carriage_return || byte != b'\r'))
                    .collect();
                let line: String = line.into_iter().map(char::from).collect();
                if pattern.matches(&line) {
                    return Some((end, line));
                }
            }
        }
        None
    }

    fn check_failure(&mut self) -> bool {
        let Some((end, line)) = self.failing_line() else {
            return false;
        };
        self.unread.drain(..=end);
        self.failure = Some(line);
        true
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    pub fn begin_wait(&mut self, pattern: &str, end: UartWaitMode) {
        self.begin_text_wait(pattern, None, end);
    }

    pub fn begin_text_wait(&mut self, pattern: &str, regex: Option<regex::Regex>, end: UartWaitMode) {
        self.begin_text_wait_with_policy(pattern, regex, end, false);
    }

    pub(crate) fn begin_text_wait_with_policy(
        &mut self, pattern: &str, regex: Option<regex::Regex>, end: UartWaitMode, first_only: bool,
    ) {
        self.failure = None;
        self.multiline = None;
        self.matched_timestamp = None;
        let mut matcher = UartLineMatch::new(pattern, regex, end, self.carriage_return, first_only);
        if self.check_failure() {
            self.active = Some(matcher);
            return;
        }
        if let Some(index) = self.unread.iter().position(|&(byte, _)| {
            matcher.receive(byte);
            matcher.matched
        }) {
            self.matched_timestamp = Some(self.unread[index].1);
            self.unread.drain(..=index);
        } else if matcher.prompt_matches() {
            // A buffered unfinished line is consumed in full, as in TerminalTester.
            self.unread.clear();
            matcher.matched = true;
        }
        self.active = Some(matcher);
    }

    pub(crate) fn begin_multiline_wait(
        &mut self, patterns: Vec<FailingUartPattern>, unfinished: bool, first_only: bool,
    ) {
        self.finish_wait();
        let mut matcher = MultilineWait {
            patterns,
            candidates: Vec::new(),
            first_only,
            unfinished: false,
            lines_seen: 0,
            line: Vec::new(),
            matched: None,
        };
        if matcher.patterns.is_empty() {
            matcher.matched = Some((String::new(), true));
        } else if !self.check_failure() {
            let end = self.terminator();
            if let Some(index) = self.unread.iter().position(|&(byte, _)| matcher.push(byte, end)) {
                self.matched_timestamp = Some(self.unread[index].1);
                self.unread.drain(..=index);
            } else {
                matcher.unfinished = unfinished;
                if matcher.finish_partial() {
                    self.unread.clear();
                }
            }
        }
        self.multiline = Some(matcher);
    }

    pub fn receive(&mut self, byte: u8) {
        self.receive_at(byte, 0);
    }

    pub(crate) fn receive_at(&mut self, byte: u8, timestamp: u128) {
        self.activity = self.activity.wrapping_add(1);
        if !self.sgr.is_empty() {
            if self.sgr.len() == 1 && byte == b'['
                || self.sgr.len() > 1 && (byte.is_ascii_digit() || byte == b';')
                    && self.sgr.len() < UART_HISTORY_BYTES
            {
                self.sgr.push(byte);
                return;
            }
            if self.sgr.len() > 1 && byte == b'm' {
                self.sgr.clear();
                return;
            }
            for pending in std::mem::take(&mut self.sgr) {
                self.receive_text_byte(pending, timestamp);
            }
        }
        if byte == 0x1b {
            self.sgr.push(byte);
        } else {
            self.receive_text_byte(byte, timestamp);
        }
    }

    fn receive_text_byte(&mut self, byte: u8, timestamp: u128) {
        let end = self.terminator();
        if byte == end && (self.active.is_some() || self.multiline.is_some())
            && !self.matched() && !self.failures.is_empty()
        {
            if self.unread.len() == UART_HISTORY_BYTES {
                self.unread.pop_front();
            }
            self.unread.push_back((byte, timestamp));
            if self.check_failure() {
                return;
            }
            self.unread.pop_back();
        }
        if let Some(matcher) = &mut self.multiline
            && self.failure.is_none()
            && matcher.push(byte, end)
        {
            self.matched_timestamp = (byte == end).then_some(timestamp);
            self.unread.clear();
            return;
        }
        if let Some(matcher) = &mut self.active
            && self.failure.is_none()
            && !matcher.matched
        {
            matcher.receive(byte);
            matcher.matched |= matcher.prompt_matches();
            if matcher.matched {
                if matcher.matched_completed_line {
                    self.matched_timestamp = Some(timestamp);
                }
                self.unread.clear();
                return;
            }
        }
        if self.unread.len() == UART_HISTORY_BYTES {
            self.unread.pop_front();
        }
        self.unread.push_back((byte, timestamp));
    }

    pub fn matched(&self) -> bool {
        self.failure.is_some() || self.active.as_ref().is_some_and(|matcher| matcher.matched)
            || self.multiline.as_ref().is_some_and(|matcher| matcher.matched.is_some())
    }

    pub(crate) fn suppress_match_captures(&self) -> bool {
        self.active.as_ref().is_some_and(|matcher| matcher.first_only && matcher.matched_completed_line)
    }

    pub(crate) fn matched_unfinished_line(&self) -> bool {
        self.active.as_ref().is_some_and(|active| active.matched && !active.matched_completed_line)
            || self.multiline.as_ref().and_then(|matcher| matcher.matched.as_ref())
                .is_some_and(|(_, completed)| !completed)
    }

    pub(crate) fn buffered_tail(&self) -> String {
        const REPORT_BYTES: usize = 512;
        let bytes: Vec<_> = self.unread.iter().skip(self.unread.len().saturating_sub(REPORT_BYTES))
            .map(|&(byte, _)| byte).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub(crate) fn matched_timestamp(&self) -> Option<u128> {
        self.matched_timestamp
    }

    pub fn matched_line(&self) -> Option<String> {
        if let Some((text, _)) = self.multiline.as_ref().and_then(|matcher| matcher.matched.as_ref()) {
            return Some(text.clone());
        }
        let matched = self.active.as_ref().filter(|active| active.matched)?;
        Some(String::from_utf8_lossy(&matched.line.iter().copied().collect::<Vec<_>>()).into_owned())
    }

    pub fn finish_wait(&mut self) {
        self.active = None;
        self.multiline = None;
        self.failure = None;
        self.matched_timestamp = None;
    }
}

#[derive(Default)]
pub struct BinaryUartHistory {
    unread: VecDeque<u8>,
    wait: Option<BinaryWait>,
}

#[derive(Clone)]
pub enum BinaryUartPattern {
    Literal(Vec<u8>),
    Regex(regex::bytes::Regex),
}

pub struct BinaryUartMatch {
    pub content: Vec<u8>,
    pub groups: Vec<Option<Vec<u8>>>,
}

struct BinaryWait {
    pattern: BinaryUartPattern,
    match_start: bool,
    start_retained: bool,
    result: Option<BinaryUartMatch>,
}

impl BinaryUartHistory {
    #[cfg(test)]
    pub fn begin_wait(&mut self, pattern: &[u8], match_start: bool) {
        self.begin_pattern(&BinaryUartPattern::Literal(pattern.to_vec()), match_start);
    }

    pub fn begin_pattern(&mut self, pattern: &BinaryUartPattern, match_start: bool) {
        self.wait = Some(BinaryWait {
            pattern: pattern.clone(),
            match_start,
            start_retained: true,
            result: None,
        });
        self.check_match(false);
    }

    fn check_match(&mut self, live: bool) {
        let Some(wait) = &mut self.wait else { return };
        if wait.result.is_some() || (wait.match_start && !wait.start_retained) {
            return;
        }
        let found = match &wait.pattern {
            BinaryUartPattern::Literal(pattern) => {
                let Some(last) = self.unread.len().checked_sub(pattern.len()) else {
                    return;
                };
                let offset = if live {
                    ((!wait.match_start || last == 0)
                        && self
                            .unread
                            .iter()
                            .rev()
                            .zip(pattern.iter().rev())
                            .all(|(left, right)| left == right))
                    .then_some(last)
                } else {
                    (0..=if wait.match_start { 0 } else { last }).find(|&offset| {
                        self.unread
                            .iter()
                            .skip(offset)
                            .take(pattern.len())
                            .eq(pattern)
                    })
                };
                offset.map(|offset| {
                    (
                        offset + pattern.len(),
                        BinaryUartMatch {
                            content: pattern.clone(),
                            groups: Vec::new(),
                        },
                    )
                })
            }
            BinaryUartPattern::Regex(pattern) => {
                let bytes = self.unread.make_contiguous();
                pattern.captures(bytes).and_then(|captures| {
                    let full = captures.get(0)?;
                    if wait.match_start && full.start() != 0 {
                        return None;
                    }
                    let groups = captures
                        .iter()
                        .skip(1)
                        .map(|group| group.map(|group| group.as_bytes().to_vec()))
                        .collect();
                    Some((
                        full.end(),
                        BinaryUartMatch {
                            content: full.as_bytes().to_vec(),
                            groups,
                        },
                    ))
                })
            }
        };
        if let Some((end, result)) = found {
            self.unread.drain(..end);
            wait.result = Some(result);
        }
    }

    pub fn receive(&mut self, byte: u8) {
        if self.unread.len() == UART_HISTORY_BYTES {
            self.unread.pop_front();
            if let Some(wait) = &mut self.wait {
                wait.start_retained = false;
            }
        }
        self.unread.push_back(byte);
        self.check_match(true);
    }

    pub fn matched(&self) -> bool {
        self.wait.as_ref().is_some_and(|wait| wait.result.is_some())
    }

    pub fn take_result(&mut self) -> Option<BinaryUartMatch> {
        self.wait.as_mut()?.result.take()
    }

    pub fn finish_wait(&mut self) {
        self.wait = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uart_line_sequences_require_consecutive_lines() {
        let mut history = UartHistory::default();
        let patterns = ["start", "value", "done"].map(|pattern| FailingUartPattern {
            pattern: pattern.into(),
            regex: None,
        });
        history.begin_multiline_wait(patterns.to_vec(), false, false);
        for byte in b"start\nintervening\nvalue\ndone\nstart\nstart\nvalue\n" {
            history.receive(*byte);
        }
        assert!(!history.matched());
        for byte in b"done\r\n" {
            history.receive_at(*byte, 41);
        }
        assert_eq!(history.matched_line().as_deref(), Some("done"));
        assert_eq!(history.matched_timestamp(), Some(41));
    }

    #[test]
    fn next_line_mismatch_keeps_both_buffered_lines() {
        let mut history = UartHistory::default();
        for &byte in b"one\ntwo\n" {
            history.receive(byte);
        }
        history.begin_text_wait_with_policy("two", None, UartWaitMode::Lf, true);
        assert!(!history.matched());
        history.finish_wait();
        for pattern in ["one", "two"] {
            history.begin_text_wait_with_policy(pattern, None, UartWaitMode::Lf, true);
            assert_eq!(history.matched_line().as_deref(), Some(pattern));
            history.finish_wait();
        }
    }

    #[test]
    fn clear_report_keeps_unfinished_text() {
        for (carriage_return, terminator) in [(false, b'\n'), (true, b'\r')] {
            let mut history = UartHistory::with_carriage_return(carriage_return);
            for byte in [b'A', terminator, b'B'] {
                history.receive(byte);
            }
            history.clear_completed_lines();
            history.receive(terminator);
            history.begin_wait("A", UartWaitMode::Lf);
            assert!(!history.matched());
            history.finish_wait();
            history.begin_wait("B", UartWaitMode::Lf);
            assert_eq!(history.matched_line().as_deref(), Some("B"));
        }
    }

    #[test]
    fn carriage_return_tester_finishes_matches_and_failing_lines() {
        let mut history = UartHistory::with_carriage_return(true);
        history.begin_wait("hello", UartWaitMode::Lf);
        for byte in b"hello\n" {
            history.receive(*byte);
        }
        assert!(!history.matched());
        history.receive(b'\r');
        assert!(history.matched());
        history.finish_wait();
        history.set_failure(FailingUartPattern {
            pattern: "failure".into(),
            regex: None,
        }, true);
        history.begin_wait("unrelated", UartWaitMode::Lf);
        for byte in b"failure\r" {
            history.receive(*byte);
        }
        assert_eq!(history.failure(), Some("failure"));
    }

    #[test]
    fn renode_binary_uart_history_keeps_exact_cursor_after_matches_and_timeouts() {
        let mut history = BinaryUartHistory::default();
        for &byte in b"ZEPHYR OS\r\n****\r\nshell" {
            history.receive(byte);
        }
        for (pattern, anchored, matched) in [
            (&b"ZEPHYR"[..], false, true),
            (&b"R OS"[..], false, false),
            (&b"OS"[..], true, false),
            (&b" OS"[..], true, true),
            (&b"****\r\nsh"[..], false, true),
            (&b"ell"[..], true, true),
        ] {
            history.begin_wait(pattern, anchored);
            assert_eq!(history.matched(), matched, "{pattern:?}");
            history.finish_wait();
        }

        history.begin_wait(b"\0\xff", true);
        history.receive(0);
        assert!(!history.matched());
        history.receive(0xff);
        assert!(history.matched());
        history.receive(b'A');
        history.finish_wait();
        history.begin_wait(b"A", true);
        assert!(history.matched());
    }

    #[test]
    fn text_waits_ignore_nul_and_treat_cr_according_to_the_line_mode() {
        let mut history = UartHistory::default();
        for &byte in b"re\0a\rdy\r\nanswer\0:tail" {
            history.receive(byte);
        }
        history.begin_wait("ready", UartWaitMode::Lf);
        assert!(history.matched());
        history.finish_wait();
        history.begin_wait("answer:", UartWaitMode::Prompt);
        assert!(history.matched());
        history.finish_wait();
        history.begin_wait("tail", UartWaitMode::Prompt);
        assert!(!history.matched());

        history.begin_wait("ready", UartWaitMode::Cr);
        for &byte in b"rea\rdy\0\r" {
            history.receive(byte);
        }
        assert!(!history.matched());
        for &byte in b"rea\0dy\r" {
            history.receive(byte);
        }
        assert!(history.matched());
    }

    #[test]
    fn prompt_prefers_finished_lines_and_consumes_the_buffered_unfinished_line() {
        let mut history = UartHistory::default();
        for &byte in b"ready\r\nready> buffered suffix" {
            history.receive(byte);
        }
        history.begin_wait("ready", UartWaitMode::Prompt);
        assert!(history.matched());
        history.finish_wait();
        history.begin_wait("ready>", UartWaitMode::Lf);
        assert!(!history.matched());
        history.finish_wait();
        history.begin_wait("ready>", UartWaitMode::Prompt);
        assert!(history.matched());
        history.finish_wait();
        history.begin_wait("buffered suffix", UartWaitMode::Prompt);
        assert!(!history.matched());
    }

    #[test]
    fn live_prompt_matches_before_termination_and_leaves_later_bytes_for_line_wait() {
        let mut history = UartHistory::default();
        for &byte in b"Rea\ndy>\nRea" {
            history.receive(byte);
        }
        history.begin_wait("Ready>", UartWaitMode::Prompt);
        assert!(!history.matched());
        for &byte in b"dy>" {
            history.receive(byte);
        }
        assert!(history.matched());
        for &byte in b"response\n" {
            history.receive(byte);
        }
        history.finish_wait();
        history.begin_wait("response", UartWaitMode::Lf);
        assert!(history.matched());
    }

    #[test]
    fn text_wait_ignores_sgr_across_buffered_and_live_output() {
        let mut history = UartHistory::default();
        for &byte in b"\x1b[1" {
            history.receive(byte);
        }
        let pattern = r"CPU:\s+(VexRiscv)";
        history.begin_text_wait(pattern, Some(regex::Regex::new(pattern).unwrap()), UartWaitMode::Lf);
        for &byte in b"mCPU\x1b[0m:\tVexRiscv\r\n" {
            history.receive(byte);
        }
        assert_eq!(history.matched_line().as_deref(), Some("CPU:\tVexRiscv"));
    }

    #[test]
    fn text_regex_wait_matches_a_finished_line_and_preserves_later_bytes() {
        let mut history = UartHistory::default();
        for &byte in b"lux: invalid\nlux: 49" {
            history.receive(byte);
        }
        let expression = r"^lux: [0-9]+$";
        history.begin_text_wait(expression, Some(regex::Regex::new(expression).unwrap()), UartWaitMode::Lf);
        assert!(!history.matched());
        for &byte in b"9\r\nnext\n" {
            history.receive(byte);
        }
        assert_eq!(history.matched_line().as_deref(), Some("lux: 499"));
        history.finish_wait();
        history.begin_wait("next", UartWaitMode::Lf);
        assert_eq!(history.matched_line().as_deref(), Some("next"));
    }

    #[test]
    fn history_consumes_through_the_first_match_and_keeps_later_bytes() {
        let mut history = UartHistory::default();
        for &byte in b"skip\nfirst\r\nlater\npartial" {
            history.receive(byte);
        }
        history.begin_wait("absent", UartWaitMode::Lf);
        assert!(!history.matched());
        history.finish_wait();
        history.begin_wait("first", UartWaitMode::Lf);
        assert!(history.matched());
        history.finish_wait();
        history.begin_wait("skip", UartWaitMode::Lf);
        assert!(!history.matched());
        history.finish_wait();
        history.begin_wait("later", UartWaitMode::Lf);
        assert!(history.matched());
        history.finish_wait();
        history.begin_wait("partial line", UartWaitMode::Lf);
        assert!(!history.matched());
        for &byte in b" line\nnext\n" {
            history.receive(byte);
        }
        assert!(history.matched());
        history.finish_wait();
        history.begin_wait("next", UartWaitMode::Lf);
        assert!(history.matched());
    }

    #[test]
    fn history_drops_oldest_bytes_when_full() {
        let mut history = UartHistory::default();
        for byte in b"old\n"
            .iter()
            .copied()
            .chain(std::iter::repeat_n(b'x', UART_HISTORY_BYTES))
            .chain(b"\nnew\n".iter().copied())
        {
            history.receive(byte);
        }
        assert_eq!(history.unread.len(), UART_HISTORY_BYTES);
        history.begin_wait("old", UartWaitMode::Lf);
        assert!(!history.matched());
        history.finish_wait();
        history.begin_wait("new", UartWaitMode::Lf);
        assert!(history.matched());
    }

    #[test]
    fn litex_receives_boot_abort_and_shell_command_in_separate_events() {
        // Renode LiteX_Ibex.robot: Should Boot / Should Display Help (UART input).
        const RX: u32 = 2;
        let mut uart = LiteXUart::new(vec![0], Some((0, 2)));
        let mut output = std::io::sink();
        uart.write(0x14, RX, &mut output).unwrap();
        for input in [b"Q".as_slice(), b"help\r"] {
            assert_eq!(uart.read(8), 1);
            assert!(!uart.interrupt_pending());
            uart.queue_input(input.to_vec());
            assert!(uart.interrupt_pending());
            for &byte in input {
                assert_eq!(uart.read(8), 0);
                assert_eq!(uart.read(0), u32::from(byte));
            }
            assert_eq!(uart.read(8), 1);
            assert_ne!(uart.read(0x10) & RX, 0);
            uart.write(0x10, RX, &mut output).unwrap();
            assert_eq!(uart.read(0x10) & RX, 0);
            assert!(!uart.interrupt_pending());
        }
    }
}
