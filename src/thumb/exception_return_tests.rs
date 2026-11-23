use std::io::sink;

use super::{Instruction, Thumb};
use crate::bus::Bus;

const RESET: u32 = 0x20;
const NMI: u32 = 0x100;
const SP: u32 = 0x800;

struct Fixture {
    cpu: Thumb,
    bus: Bus,
    pc: u32,
    regs: [u32; 32],
}

impl Fixture {
    fn new(reset_code: &[u16], nmi_code: &[u16]) -> Self {
        let mut bus = Bus::default();
        bus.map_ram(0, 0x1000).unwrap();
        bus.map_nvic(0xff, None).unwrap();
        bus.map_systick().unwrap();
        for (address, value) in [(0, SP), (4, RESET | 1), (8, NMI | 1), (12, 0x181)] {
            bus.write(address, 4, value, &mut sink()).unwrap();
        }
        for (address, program) in [(RESET, reset_code), (NMI, nmi_code)] {
            for (index, word) in program.iter().enumerate() {
                bus.write(address + index as u32 * 2, 2, u32::from(*word), &mut sink())
                    .unwrap();
            }
        }
        let mut cpu = Thumb::new(Some(0));
        cpu.thumb2 = true;
        let mut regs = [0; 32];
        let pc = cpu.reset(&mut bus, &mut regs).unwrap();
        Self { cpu, bus, pc, regs }
    }

    fn step(&mut self) -> Instruction {
        self.cpu
            .service_interrupts(&mut self.pc, &mut self.regs, &mut self.bus)
            .unwrap();
        self.cpu
            .execute_instruction(&mut self.pc, &mut self.regs, &mut self.bus, &mut sink())
            .unwrap()
    }

    fn enter_nmi(&mut self) {
        assert!(matches!(self.step(), Instruction::Retired));
        assert_eq!(self.pc, RESET);
        self.bus
            .write(0xe000_ed04, 4, 1 << 31, &mut sink())
            .unwrap();
        assert!(matches!(self.step(), Instruction::Retired)); // Handler NOP.
        assert_eq!(self.pc, NMI + 2);
        assert_eq!(self.regs[14], 0xffff_fff9);
    }
}

#[test]
fn renode_exc_return_thread_bx_installs_pc_before_invalid_state_fault() {
    // mov r0, 0xffffffb8; bx r0
    let mut fixture = Fixture::new(&[0xf06f, 0x0047, 0x4700], &[0xe7fe]);
    assert!(matches!(fixture.step(), Instruction::Retired));
    assert!(matches!(fixture.step(), Instruction::Retired));
    assert_eq!(fixture.pc, 0xffff_ffb8);
    assert_eq!(fixture.regs[13], SP);
    assert_eq!(fixture.cpu.active_exception, 0);

    // Fetching thereafter records INVSTATE at the branch destination.
    assert!(matches!(fixture.step(), Instruction::Fault));
    assert_eq!(fixture.pc, 0x180);
    assert_eq!(fixture.bus.read::<4>(0xe000_ed28).unwrap(), 1 << 17);
    assert_eq!(fixture.bus.read::<4>(SP - 8).unwrap(), 0xffff_ffb8);
}

#[test]
fn renode_exc_return_nmi_bx_lr_consumes_original_frame() {
    let mut fixture = Fixture::new(&[0xe7fe], &[0xbf00, 0x4770]);
    fixture.enter_nmi();
    assert!(matches!(fixture.step(), Instruction::Retired));
    assert_eq!(fixture.pc, RESET);
    assert_eq!(fixture.regs[13], SP);
    assert_eq!(fixture.cpu.active_exception, 0);
}

#[test]
fn renode_exc_return_nmi_blx_lr_is_an_ordinary_linking_branch() {
    let mut fixture = Fixture::new(&[0xe7fe], &[0xbf00, 0x47f0]);
    fixture.enter_nmi();
    assert!(matches!(fixture.step(), Instruction::Retired));
    assert_eq!(fixture.pc, 0xffff_fff8);
    assert_eq!(fixture.regs[14], 0x105);
    assert_eq!(fixture.regs[13], SP - 32);
    assert_eq!(fixture.cpu.active_exception, 2);
}

#[test]
fn renode_exc_return_bx_without_trustzone_is_an_ordinary_branch() {
    // mov r0, 0xfeffffff; bx r0
    let mut fixture = Fixture::new(&[0xf06f, 0x7080, 0x4700], &[0xe7fe]);
    assert!(matches!(fixture.step(), Instruction::Retired));
    assert!(matches!(fixture.step(), Instruction::Retired));
    assert_eq!(fixture.pc, 0xfeff_fffe);
    assert_eq!(fixture.regs[13], SP);
    assert_eq!(fixture.cpu.active_exception, 0);
}
