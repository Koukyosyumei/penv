use crate::{bus::STM32_STEP_HZ, stm32_uart::Stm32Irq};

const FORMAT_12H: u32 = 1 << 6;
const ALARM_ENABLE: u32 = 1 << 8;
const WAKE_ENABLE: u32 = 1 << 10;
const INIT: u32 = 1 << 7;
const SYNC: u32 = 1 << 5;

pub struct Stm32Rtc {
    pub base: u32,
    pub routes: [Option<Stm32Irq>; 2],
    frequency: u32,
    phase: u64,
    async_count: u32,
    subsecond: u32,
    wake_count: u64,
    control: u32,
    status: u32,
    prescaler: u32,
    wakeup_reload: u32,
    masks: [u32; 2],
    alarms: [u32; 2],
    backup: [u32; 20],
    calendar: Calendar,
    key: u8,
    errata: bool,
}

struct Calendar {
    second: u32,
    minute: u32,
    hour: u32,
    day: u32,
    month: u32,
    year: u32,
    weekday: u32,
}

impl Stm32Rtc {
    pub const SIZE: u32 = 0x400;

    pub fn new(base: u32, frequency: u32, errata: bool) -> Self {
        Self {
            base,
            routes: [None; 2],
            frequency,
            phase: 0,
            async_count: 0,
            subsecond: 255,
            wake_count: 0,
            control: 0,
            status: 0,
            prescaler: 0x007f_00ff,
            wakeup_reload: 0xffff,
            masks: [0; 2],
            alarms: [0; 2],
            backup: [0; 20],
            calendar: Calendar {
                second: 0,
                minute: 0,
                hour: 0,
                day: 1,
                month: 1,
                year: 20,
                weekday: 1,
            },
            key: 0,
            errata,
        }
    }

    pub fn contains(&self, address: u32) -> bool {
        address.wrapping_sub(self.base) < Self::SIZE
    }

    pub fn read(&mut self, address: u32) -> u32 {
        match address - self.base {
            0 => self.calendar.time(self.control & FORMAT_12H != 0),
            4 => self.calendar.date(),
            8 => self.control | (1 << 5),
            0x0c => {
                let status = (self.status & !INIT)
                    | ((self.status & INIT) >> 1)
                    | ((!self.control >> 8) & 7)
                    | (u32::from(self.calendar.year != 0) << 4);
                self.status |= SYNC;
                status
            }
            0x10 => self.prescaler,
            0x14 if self.errata => self.wakeup_reload,
            0x14 => self.wakeup_reload.wrapping_add(1) & 0xffff,
            0x1c => self.alarms[0],
            0x20 => self.alarms[1],
            0x28 => self.subsecond,
            0x44 => self.masks[0],
            0x48 => self.masks[1],
            offset @ 0x50..=0x9c => self.backup[((offset - 0x50) / 4) as usize],
            _ => 0,
        }
    }

    pub fn write(&mut self, address: u32, value: u32) {
        let offset = address - self.base;
        if offset == 0x24 {
            self.key = match (self.key, value as u8) {
                (0 | 2, 0xca) => 1,
                (1, 0x53) => 2,
                _ => 0,
            };
            return;
        }
        if let 0x50..=0x9c = offset {
            self.backup[((offset - 0x50) / 4) as usize] = value;
            return;
        }
        if offset == 0x0c {
            self.status &= value | !(SYNC | 0xff00);
        }
        if self.key != 2 {
            return;
        }
        match offset {
            0 if self.status & INIT != 0 => self
                .calendar
                .set_time(value, self.control & FORMAT_12H != 0),
            4 if self.status & INIT != 0 => self.calendar.set_date(value),
            8 => {
                if (value ^ self.control) & (WAKE_ENABLE | 7) != 0 {
                    self.wake_count = 0;
                }
                self.control = value & 0x00ff_ffff;
            }
            0x0c => self.status = (self.status & !INIT) | (value & INIT),
            0x10 => {
                self.prescaler = value & 0x007f_7fff;
                self.subsecond = self.prescaler & 0x7fff;
                self.async_count = 0;
            }
            0x14 if self.control & WAKE_ENABLE == 0 => self.wakeup_reload = value & 0xffff,
            0x1c | 0x20 => {
                let alarm = ((offset - 0x1c) / 4) as usize;
                if self.control & (ALARM_ENABLE << alarm) == 0 {
                    self.alarms[alarm] = value;
                }
            }
            0x44 | 0x48 => {
                let alarm = ((offset - 0x44) / 4) as usize;
                if self.control & (ALARM_ENABLE << alarm) == 0 {
                    self.masks[alarm] = value & 0x0f00_7fff;
                }
            }
            _ => {}
        }
    }

    pub fn tick(&mut self) -> u32 {
        self.phase += u64::from(self.frequency);
        if self.phase >= STM32_STEP_HZ {
            let cycles = self.phase / STM32_STEP_HZ;
            self.phase %= STM32_STEP_HZ;
            for _ in 0..cycles {
                self.advance();
            }
        }
        let flags = ((self.status & (self.control >> 4)) >> 8) & 7;
        u32::from(flags & 3 != 0) | (u32::from(flags & 4 != 0) << 1)
    }

    fn advance(&mut self) {
        let asynchronous = (self.prescaler >> 16) + 1;
        let synchronous = (self.prescaler & 0x7fff) + 1;
        if self.status & INIT == 0 {
            self.async_count += 1;
            if self.async_count >= asynchronous {
                self.async_count = 0;
                let new_second = self.subsecond == 0;
                if new_second {
                    self.subsecond = synchronous - 1;
                    self.calendar.advance();
                } else {
                    self.subsecond -= 1;
                }
                self.check_alarms(new_second);
            }
        }
        if self.control & WAKE_ENABLE != 0 {
            let select = self.control & 7;
            let divider = if select < 4 {
                16 >> select
            } else {
                asynchronous * synchronous
            };
            let reload = (self.wakeup_reload + 1) + if select >= 6 { 0x10000 } else { 0 };
            self.wake_count += 1;
            if self.wake_count >= u64::from(divider) * u64::from(reload) {
                self.wake_count = 0;
                self.status |= 1 << 10;
            }
        }
    }

    fn check_alarms(&mut self, new_second: bool) {
        let time = self.calendar.time(self.control & FORMAT_12H != 0);
        for (index, alarm) in self.alarms.iter().copied().enumerate() {
            if self.control & (ALARM_ENABLE << index) == 0 {
                continue;
            }
            let bits = self.masks[index] >> 24;
            let mask = (1 << bits) - 1;
            if (bits == 0 && !new_second)
                || (bits != 0 && (self.masks[index] ^ self.subsecond) & mask != 0)
            {
                continue;
            }
            let day = if alarm & (1 << 30) == 0 {
                bcd(self.calendar.day)
            } else {
                self.calendar.weekday
            };
            let current = time | (day << 24);
            let significant = [0x7f, 0x7f00, 0x007f_0000, 0x3f00_0000];
            if significant.iter().enumerate().all(|(field, mask)| {
                alarm & (0x80 << (field * 8)) != 0 || (alarm ^ current) & mask == 0
            }) {
                self.status |= 1 << (8 + index);
            }
        }
    }
}

impl Calendar {
    fn advance(&mut self) {
        self.second += 1;
        if self.second < 60 {
            return;
        }
        self.second = 0;
        self.minute += 1;
        if self.minute < 60 {
            return;
        }
        self.minute = 0;
        self.hour += 1;
        if self.hour < 24 {
            return;
        }
        self.hour = 0;
        self.weekday = self.weekday % 7 + 1;
        self.day += 1;
        if self.day <= self.days_in_month() {
            return;
        }
        self.day = 1;
        self.month += 1;
        if self.month > 12 {
            self.month = 1;
            self.year = (self.year + 1) % 100;
        }
    }

    fn days_in_month(&self) -> u32 {
        match self.month {
            2 => 28 + u32::from(self.year.is_multiple_of(4)),
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        }
    }

    fn time(&self, twelve_hours: bool) -> u32 {
        let hour = if twelve_hours {
            (self.hour + 11) % 12 + 1
        } else {
            self.hour
        };
        bcd(self.second)
            | (bcd(self.minute) << 8)
            | (bcd(hour) << 16)
            | (u32::from(twelve_hours && self.hour >= 12) << 22)
    }

    fn date(&self) -> u32 {
        bcd(self.day) | (bcd(self.month) << 8) | (self.weekday << 13) | (bcd(self.year) << 16)
    }

    fn set_time(&mut self, value: u32, twelve_hours: bool) {
        let hour = from_bcd((value >> 16) & 0x3f);
        self.second = from_bcd(value & 0x7f).min(59);
        self.minute = from_bcd((value >> 8) & 0x7f).min(59);
        self.hour = if twelve_hours {
            hour.clamp(1, 12) % 12 + if value & (1 << 22) != 0 { 12 } else { 0 }
        } else {
            hour.min(23)
        };
    }

    fn set_date(&mut self, value: u32) {
        self.year = from_bcd((value >> 16) & 0xff).min(99);
        self.month = from_bcd((value >> 8) & 0x1f).clamp(1, 12);
        self.day = from_bcd(value & 0x3f).clamp(1, self.days_in_month());
        self.weekday = ((value >> 13) & 7).max(1);
    }
}

fn bcd(value: u32) -> u32 {
    ((value / 10) << 4) | (value % 10)
}

fn from_bcd(value: u32) -> u32 {
    (value >> 4) * 10 + (value & 0xf)
}
