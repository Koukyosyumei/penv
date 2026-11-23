use std::time::{SystemTime, UNIX_EPOCH};

use crate::bus::CORTEX_M_STEP_HZ;

#[derive(Clone, Copy, Default)]
pub enum RtcMode {
    #[default]
    Epoch,
    HostTimeLocal,
    HostTimeUtc,
}

impl RtcMode {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "Epoch" => Ok(Self::Epoch),
            "HostTimeLocal" => Ok(Self::HostTimeLocal),
            "HostTimeUTC" => Ok(Self::HostTimeUtc),
            _ => Err(format!("unknown RTC mode {value:?}")),
        }
    }
}

pub struct RtcClock {
    created_at: i64,
    origin: i64,
}

impl Default for RtcClock {
    fn default() -> Self {
        let created_at = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(duration) => duration.as_secs() as i64,
            Err(error) => {
                -(error.duration().as_secs() as i64)
                    - i64::from(error.duration().subsec_nanos() != 0)
            }
        };
        Self {
            created_at,
            origin: 0,
        }
    }
}

impl RtcClock {
    pub fn set_mode(&mut self, mode: RtcMode) -> Result<(), String> {
        self.origin = match mode {
            RtcMode::Epoch => 0,
            RtcMode::HostTimeUtc => self.created_at,
            RtcMode::HostTimeLocal => self.created_at + local_offset(self.created_at)?,
        };
        Ok(())
    }

    pub fn date_time(&self, steps: u64) -> String {
        let seconds = self.origin + (steps / CORTEX_M_STEP_HZ) as i64;
        let (year, month, day) = civil_date(seconds.div_euclid(86_400));
        let within_day = seconds.rem_euclid(86_400);
        let hour = within_day / 3600;
        let minute = within_day / 60 % 60;
        let second = within_day % 60;
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
    }
}

fn civil_date(days: i64) -> (i64, i64, i64) {
    // Start Gregorian 400-year eras on March 1 so leap days end each year.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = era * 400 + year_of_era;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_month + 2) / 5 + 1;
    let month = if march_month < 10 {
        march_month + 3
    } else {
        march_month - 9
    };
    (year + i64::from(month <= 2), month, day)
}

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
fn local_offset(seconds: i64) -> Result<i64, String> {
    use std::{
        ffi::{c_char, c_int, c_long},
        mem::MaybeUninit,
    };

    #[repr(C)]
    struct LocalTime {
        calendar: [c_int; 9],
        offset: c_long,
        zone: *const c_char,
    }
    unsafe extern "C" {
        fn localtime_r(seconds: *const c_long, result: *mut LocalTime) -> *mut LocalTime;
    }
    let mut result = MaybeUninit::<LocalTime>::uninit();
    // SAFETY: Both arguments point to valid storage for Linux's time_t and tm.
    if unsafe { localtime_r(&seconds, result.as_mut_ptr()) }.is_null() {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: localtime_r initialized the structure on success.
    Ok(unsafe { result.assume_init() }.offset)
}

#[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
fn local_offset(_: i64) -> Result<i64, String> {
    Err("HostTimeLocal requires a 64-bit Linux host".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_utc_use_the_same_virtual_elapsed_time() {
        let mut clock = RtcClock {
            created_at: 1_709_164_799,
            origin: 0,
        };
        assert_eq!(clock.date_time(0), "1970-01-01 00:00:00");
        clock.set_mode(RtcMode::HostTimeUtc).unwrap();
        assert_eq!(clock.date_time(0), "2024-02-28 23:59:59");
        assert_eq!(clock.date_time(CORTEX_M_STEP_HZ), "2024-02-29 00:00:00");
        clock.set_mode(RtcMode::Epoch).unwrap();
        assert_eq!(clock.date_time(CORTEX_M_STEP_HZ), "1970-01-01 00:00:01");
    }
}
