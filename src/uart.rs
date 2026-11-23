use std::{
    collections::VecDeque,
    io::{self, Write},
};

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

pub struct LiteXUart {
    pub base: u32,
    receive: VecDeque<u8>,
    pending: u32,
    enabled: u32,
}

impl LiteXUart {
    pub const SIZE: u32 = 0x100;

    pub fn new(base: u32) -> Self {
        Self {
            base,
            receive: VecDeque::new(),
            pending: 1,
            enabled: 0,
        }
    }

    pub fn offset(&self, address: u32, width: usize) -> Option<u32> {
        let offset = address.wrapping_sub(self.base);
        (offset < Self::SIZE && matches!(width, 1 | 4)).then_some(offset)
    }

    pub fn queue_input(&mut self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.receive.extend(bytes);
            self.pending |= 2;
        }
    }

    pub fn interrupt_pending(&self) -> bool {
        self.enabled & self.pending != 0
    }

    pub fn read(&mut self, offset: u32) -> u32 {
        match offset {
            0 => u32::from(self.receive.pop_front().unwrap_or(0)),
            8 => u32::from(self.receive.is_empty()),
            0x10 => self.pending,
            0x14 => self.enabled,
            _ => 0,
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, output: &mut impl Write) -> io::Result<()> {
        match offset {
            0 => {
                output.write_all(&[value as u8])?;
                self.pending |= 1;
            }
            0x10 => self.pending &= !value,
            0x14 => self.enabled = value & 3,
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
    tail: VecDeque<u8>,
    contains: bool,
    matched: bool,
}

impl UartLineMatch {
    fn new(pattern: &str, end: UartWaitMode) -> Self {
        Self {
            end: match end {
                UartWaitMode::Lf | UartWaitMode::Prompt => b'\n',
                UartWaitMode::Cr => b'\r',
            },
            prompt: matches!(end, UartWaitMode::Prompt),
            pattern: pattern.as_bytes().to_vec(),
            tail: VecDeque::new(),
            contains: false,
            matched: false,
        }
    }

    fn receive(&mut self, byte: u8) {
        // TerminalTester text mode discards NUL and, for LF-delimited
        // lines, CR before matching. Raw output and other watchers keep them.
        if self.matched || byte == 0 || (self.end == b'\n' && byte == b'\r') {
            return;
        }
        if byte == self.end {
            self.matched = self.contains;
            self.tail.clear();
            self.contains = false;
        } else if !self.contains {
            if self.tail.len() == self.pattern.len() {
                self.tail.pop_front();
            }
            self.tail.push_back(byte);
            self.contains = self.tail.iter().eq(self.pattern.iter());
        }
    }
}

pub const UART_HISTORY_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub struct UartHistory {
    unread: VecDeque<u8>,
    active: Option<UartLineMatch>,
}

impl UartHistory {
    pub fn begin_wait(&mut self, pattern: &str, end: UartWaitMode) {
        let mut matcher = UartLineMatch::new(pattern, end);
        if let Some(index) = self.unread.iter().position(|&byte| {
            matcher.receive(byte);
            matcher.matched
        }) {
            self.unread.drain(..=index);
        } else if matcher.prompt && matcher.contains {
            // A buffered unfinished line is consumed in full, as in TerminalTester.
            self.unread.clear();
            matcher.matched = true;
        }
        self.active = Some(matcher);
    }

    pub fn receive(&mut self, byte: u8) {
        if let Some(matcher) = &mut self.active
            && !matcher.matched
        {
            matcher.receive(byte);
            matcher.matched |= matcher.prompt && matcher.contains;
            if matcher.matched {
                self.unread.clear();
                return;
            }
        }
        if self.unread.len() == UART_HISTORY_BYTES {
            self.unread.pop_front();
        }
        self.unread.push_back(byte);
    }

    pub fn matched(&self) -> bool {
        self.active.as_ref().is_some_and(|matcher| matcher.matched)
    }

    pub fn finish_wait(&mut self) {
        self.active = None;
    }
}

#[derive(Default)]
pub struct BinaryUartHistory {
    unread: VecDeque<u8>,
    wait: Option<BinaryWait>,
}

struct BinaryWait {
    pattern: Vec<u8>,
    match_start: bool,
    start_retained: bool,
    matched: bool,
}

impl BinaryUartHistory {
    pub fn begin_wait(&mut self, pattern: &[u8], match_start: bool) {
        let last = self.unread.len().checked_sub(pattern.len());
        let offset = last.and_then(|last| {
            (0..=if match_start { 0 } else { last }).find(|&offset| {
                self.unread
                    .iter()
                    .skip(offset)
                    .take(pattern.len())
                    .eq(pattern)
            })
        });
        if let Some(offset) = offset {
            self.unread.drain(..offset + pattern.len());
        }
        self.wait = Some(BinaryWait {
            pattern: pattern.to_vec(),
            match_start,
            start_retained: true,
            matched: offset.is_some(),
        });
    }

    pub fn receive(&mut self, byte: u8) {
        if self.unread.len() == UART_HISTORY_BYTES {
            self.unread.pop_front();
            if let Some(wait) = &mut self.wait {
                wait.start_retained = false;
            }
        }
        self.unread.push_back(byte);
        if let Some(wait) = &mut self.wait
            && !wait.matched
            && self.unread.len() >= wait.pattern.len()
            && (!wait.match_start
                || (wait.start_retained && self.unread.len() == wait.pattern.len()))
            && self
                .unread
                .iter()
                .rev()
                .zip(wait.pattern.iter().rev())
                .all(|(left, right)| left == right)
        {
            wait.matched = true;
            self.unread.clear();
        }
    }

    pub fn matched(&self) -> bool {
        self.wait.as_ref().is_some_and(|wait| wait.matched)
    }

    pub fn finish_wait(&mut self) {
        self.wait = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
