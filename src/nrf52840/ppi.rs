// Programmable channels and group masks. Fixed radio/timer channels require additional models.
#[derive(Default)]
pub(super) struct Ppi {
    enabled: u32,
    endpoints: [(u32, u32); 20],
    forks: [u32; 20],
    groups: [u32; 6],
}

impl Ppi {
    pub(super) fn read(&self, offset: u32) -> Option<u32> {
        Some(match offset {
            0x000..=0x02c => 0,
            0x500 | 0x504 | 0x508 => self.enabled,
            0x510..=0x5ac => {
                let (event, task) = self.endpoints[((offset - 0x510) / 8) as usize];
                if offset & 4 == 0 { event } else { task }
            }
            0x800..=0x814 => self.groups[((offset - 0x800) / 4) as usize],
            0x910..=0x95c => self.forks[((offset - 0x910) / 4) as usize],
            _ => return None,
        })
    }

    pub(super) fn write(
        &mut self,
        offset: u32,
        value: u32,
        is_event: impl Fn(u32) -> bool,
        is_task: impl Fn(u32) -> bool,
    ) -> bool {
        const PROGRAMMABLE: u32 = (1 << 20) - 1;
        match offset {
            0x000..=0x02c => {
                if value & 1 != 0 {
                    let mask = self.groups[(offset / 8) as usize];
                    if offset & 4 == 0 {
                        self.enabled |= mask;
                    } else {
                        self.enabled &= !mask;
                    }
                }
            }
            0x500 | 0x504 if value & !PROGRAMMABLE == 0 => {
                self.enabled = if offset == 0x504 {
                    self.enabled | value
                } else {
                    value
                };
            }
            0x508 => self.enabled &= !value,
            0x510..=0x5ac => {
                let endpoints = &mut self.endpoints[((offset - 0x510) / 8) as usize];
                if offset & 4 == 0 && (value == 0 || is_event(value)) {
                    endpoints.0 = value;
                } else if offset & 4 != 0 && (value == 0 || is_task(value)) {
                    endpoints.1 = value;
                } else {
                    return false;
                }
            }
            0x800..=0x814 if value & !PROGRAMMABLE == 0 => {
                self.groups[((offset - 0x800) / 4) as usize] = value
            }
            0x910..=0x95c if value == 0 || is_task(value) => {
                self.forks[((offset - 0x910) / 4) as usize] = value
            }
            _ => return false,
        }
        true
    }

    pub(super) fn tasks(&self, event: u32, output: &mut Vec<u32>) {
        for (i, &(source, task)) in self.endpoints.iter().enumerate() {
            if self.enabled & (1 << i) != 0 && source == event {
                output.extend(
                    [task, self.forks[i]]
                        .into_iter()
                        .filter(|&address| address != 0),
                );
            }
        }
    }
}
