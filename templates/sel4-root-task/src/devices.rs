//! The device half of the root task: every `dev` grant the program declares becomes a capability at boot (a mapped
//! register window, an interrupt line bound to the one notification), and every device ask is answered against the
//! grants with raw I/O. A window or line the program did not grant is never claimed, so seL4 refuses it.

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::ptr;
use orbital_core_base::os::{OsError, OsErrorKind, OsOp};
use orbital_core_base::os_grants::{dev_target, DevKind, DevResource, DevTarget, OsGrants, OsRequirement};
use orbital_core_base::value::{Value, ValueMap};
use orbital_core_runtime::resumable::{CallSite, IoAnswer, IoRequest};
use sel4::CapTypeForObjectOfFixedSize;

const GRANULE_SIZE: usize = sel4::FrameObjectType::GRANULE.bytes();

/// Register windows this image can map: one page each.
const MAX_WINDOWS: usize = 4;

#[repr(C, align(4096))]
struct WindowPlaceholders(#[allow(dead_code)] [[u8; GRANULE_SIZE]; MAX_WINDOWS]);

static mut WINDOW_PLACEHOLDERS: WindowPlaceholders = WindowPlaceholders([[0; GRANULE_SIZE]; MAX_WINDOWS]);

struct Window {
    base: u64,
    length: u64,
    vaddr: usize,
}

struct Line {
    irq: u64,
    bit: u64,
    handler: sel4::cap::IrqHandler,
}

struct Waiter {
    irq: u64,
    site: CallSite,
    key: String,
}

pub struct Devices {
    windows: Vec<Window>,
    lines: Vec<Line>,
    notification: sel4::cap::Notification,
    waiting: RefCell<Vec<Waiter>>,
    pending: RefCell<u64>,
}

struct Slots(core::iter::Map<core::ops::Range<usize>, fn(usize) -> sel4::init_thread::Slot>);

impl Slots {
    fn next(&mut self) -> sel4::init_thread::Slot {
        self.0.next().expect("the root task has an empty slot left")
    }
}

impl Devices {
    /// Claim every window and line `grants` names.
    pub fn claim(bootinfo: &sel4::BootInfoPtr, grants: &OsGrants) -> Self {
        let mut slots = Slots(bootinfo.empty().range().map(sel4::init_thread::Slot::from_index as fn(usize) -> _));
        let kernel_ut = largest_kernel_untyped(bootinfo);
        let cnode = sel4::init_thread::slot::CNODE.cap();

        let notification_slot = slots.next().downcast::<sel4::cap_type::Notification>();
        kernel_ut
            .untyped_retype(&sel4::ObjectBlueprint::Notification, &cnode.absolute_cptr_for_self(), notification_slot.index(), 1)
            .expect("the interrupt notification is made");
        let notification = notification_slot.cap();

        let mut windows = Vec::new();
        let mut lines = Vec::new();
        for grant in &grants.dev {
            match dev_target(&grant.target) {
                Some(DevTarget::Mmio { base, length }) => {
                    assert!(length as usize <= GRANULE_SIZE, "{}: a window larger than one page is not mapped yet", grant.target);
                    assert!(windows.len() < MAX_WINDOWS, "{}: this image maps at most {MAX_WINDOWS} windows", grant.target);
                    let vaddr = map_window(bootinfo, &mut slots, base as usize, windows.len());
                    windows.push(Window { base, length, vaddr });
                }
                Some(DevTarget::Irq { line }) => {
                    let bit = 1u64 << lines.len();
                    let handler = slots.next().downcast::<sel4::cap_type::IrqHandler>().cap();
                    sel4::init_thread::slot::IRQ_CONTROL
                        .cap()
                        .irq_control_get(line as sel4::Word, &cnode.absolute_cptr(handler))
                        .expect("the granted interrupt line is claimed");
                    let badged = slots.next().downcast::<sel4::cap_type::Notification>().cap();
                    cnode
                        .absolute_cptr(badged)
                        .mint(&cnode.absolute_cptr(notification), sel4::CapRights::all(), bit as sel4::Word)
                        .expect("the line's badge is minted");
                    handler.irq_handler_set_notification(badged).expect("the line signals the notification");
                    lines.push(Line { irq: line, bit, handler });
                }
                None => panic!("{}: not a device target", grant.target),
            }
        }
        Devices { windows, lines, notification, waiting: RefCell::new(Vec::new()), pending: RefCell::new(0) }
    }

    /// Answer one `os/` ask: the device operators against `grants`, anything else is not this host's.
    pub fn answer(&self, grants: &OsGrants, request: &IoRequest) -> IoAnswer {
        let IoRequest::Os { op, args, key, site, .. } = request else {
            return IoAnswer::Os(Err(OsError::new(OsErrorKind::Invalid, "not an os request")));
        };
        let answered = match op {
            OsOp::MmioRead | OsOp::MmioWrite => self.register(grants, *op, args),
            OsOp::IrqAck => self.ack(grants, args),
            OsOp::IrqWait => match (key, site) {
                (Some(key), Some(site)) => return self.wait(grants, args, key, site),
                _ => Err(OsError::new(OsErrorKind::Invalid, "os/irq-wait: a keyed call with a completion route")),
            },
            other => Err(OsError::new(OsErrorKind::Unsupported, alloc::format!("{}: this host answers only device operators", other.name()))),
        };
        if let Err(error) = &answered {
            sel4::debug_println!("refused {}: {}", error.kind.as_str(), error.message);
        }
        IoAnswer::Os(answered)
    }

    fn register(&self, grants: &OsGrants, op: OsOp, args: &[Value]) -> Result<Value, OsError> {
        let name = op.name();
        let (base, offset, width) = (whole(args, 0, name)?, whole(args, 1, name)?, whole(args, 2, name)?);
        let kind = if op == OsOp::MmioRead { DevKind::Read } else { DevKind::Write };
        let need = OsRequirement::Dev { resource: DevResource::Mmio { base, offset, width }, kind };
        if !grants.allows(&need) {
            return Err(OsError::new(OsErrorKind::Denied, alloc::format!("{name}: {base:#x}+{offset:#x} is not granted")));
        }
        let at = base + offset;
        let window = self
            .windows
            .iter()
            .find(|w| at >= w.base && at + width <= w.base + w.length)
            .ok_or_else(|| OsError::new(OsErrorKind::Denied, alloc::format!("{name}: {at:#x} lies in no claimed window")))?;
        let address = window.vaddr + (at - window.base) as usize;
        if op == OsOp::MmioRead {
            let value = unsafe {
                match width {
                    1 => ptr::read_volatile(address as *const u8) as u64,
                    2 => ptr::read_volatile(address as *const u16) as u64,
                    4 => ptr::read_volatile(address as *const u32) as u64,
                    8 => ptr::read_volatile(address as *const u64),
                    _ => return Err(OsError::new(OsErrorKind::Invalid, alloc::format!("{name}: width {width} is not 1, 2, 4 or 8"))),
                }
            };
            Ok(record([("value", Value::Number(value as f64))]))
        } else {
            let value = whole(args, 3, name)?;
            unsafe {
                match width {
                    1 => ptr::write_volatile(address as *mut u8, value as u8),
                    2 => ptr::write_volatile(address as *mut u16, value as u16),
                    4 => ptr::write_volatile(address as *mut u32, value as u32),
                    8 => ptr::write_volatile(address as *mut u64, value),
                    _ => return Err(OsError::new(OsErrorKind::Invalid, alloc::format!("{name}: width {width} is not 1, 2, 4 or 8"))),
                }
            }
            Ok(record([]))
        }
    }

    fn line(&self, grants: &OsGrants, args: &[Value], kind: DevKind, name: &str) -> Result<&Line, OsError> {
        let irq = whole(args, 0, name)?;
        if !grants.allows(&OsRequirement::Dev { resource: DevResource::Irq { line: irq }, kind }) {
            return Err(OsError::new(OsErrorKind::Denied, alloc::format!("{name}: line {irq} is not granted")));
        }
        self.lines
            .iter()
            .find(|l| l.irq == irq)
            .ok_or_else(|| OsError::new(OsErrorKind::Denied, alloc::format!("{name}: line {irq} is not claimed")))
    }

    fn ack(&self, grants: &OsGrants, args: &[Value]) -> Result<Value, OsError> {
        let line = self.line(grants, args, DevKind::Ack, "os/irq-ack")?;
        line.handler
            .irq_handler_ack()
            .map_err(|e| OsError::new(OsErrorKind::Io, alloc::format!("os/irq-ack: {e:?}")))?;
        Ok(record([]))
    }

    /// A line that already fired answers at once; otherwise the call completes when it fires.
    fn wait(&self, grants: &OsGrants, args: &[Value], key: &str, site: &CallSite) -> IoAnswer {
        let line = match self.line(grants, args, DevKind::Wait, "os/irq-wait") {
            Ok(line) => line,
            Err(error) => return IoAnswer::Os(Err(error)),
        };
        let mut pending = self.pending.borrow_mut();
        if *pending & line.bit != 0 {
            *pending &= !line.bit;
            return IoAnswer::Os(Ok(record([("irq", Value::Number(line.irq as f64))])));
        }
        self.waiting.borrow_mut().push(Waiter { irq: line.irq, site: site.clone(), key: String::from(key) });
        IoAnswer::ServiceDeferred
    }

    /// Block until a granted line fires; answer each waiting call on the lines that did, keep the rest pending.
    pub fn wait_for_interrupts(&self) -> Vec<(CallSite, String, Value)> {
        let (_, badge) = self.notification.wait();
        let mut fired = Vec::new();
        let mut pending = self.pending.borrow_mut();
        for line in self.lines.iter().filter(|l| badge as u64 & l.bit != 0) {
            let mut waiting = self.waiting.borrow_mut();
            let before = waiting.len();
            waiting.retain(|w| {
                if w.irq == line.irq {
                    fired.push((w.site.clone(), w.key.clone(), record([("irq", Value::Number(line.irq as f64))])));
                    false
                } else {
                    true
                }
            });
            if waiting.len() == before {
                *pending |= line.bit;
            }
        }
        fired
    }
}

fn record<const N: usize>(fields: [(&str, Value); N]) -> Value {
    Value::Object(ValueMap::from_iter(fields.into_iter().map(|(k, v)| (String::from(k), v))))
}

fn whole(args: &[Value], at: usize, name: &str) -> Result<u64, OsError> {
    match args.get(at) {
        Some(Value::Number(n)) if *n >= 0.0 && *n == (*n as u64) as f64 => Ok(*n as u64),
        _ => Err(OsError::new(OsErrorKind::Invalid, alloc::format!("{name}: argument {at} is not a whole number"))),
    }
}

fn largest_kernel_untyped(bootinfo: &sel4::BootInfoPtr) -> sel4::cap::Untyped {
    let (index, _) = bootinfo
        .untyped_list()
        .iter()
        .enumerate()
        .filter(|(_, desc)| !desc.is_device())
        .max_by_key(|(_, desc)| desc.size_bits())
        .expect("seL4 hands the root task kernel memory");
    bootinfo.untyped().index(index).cap()
}

/// Carve the page at `paddr` out of the device memory that holds it, and map it over placeholder page `index`.
fn map_window(bootinfo: &sel4::BootInfoPtr, slots: &mut Slots, paddr: usize, index: usize) -> usize {
    let cnode = sel4::init_thread::slot::CNODE.cap();
    let (ut_index, desc) = bootinfo
        .untyped_list()
        .iter()
        .enumerate()
        .find(|(_, desc)| (desc.paddr()..desc.paddr() + (1 << desc.size_bits())).contains(&paddr))
        .unwrap_or_else(|| panic!("{paddr:#x}: no untyped memory holds this window"));
    assert!(desc.is_device(), "{paddr:#x}: the window is not device memory");
    let device_ut = bootinfo.untyped().index(ut_index).cap();
    let (keep, spare) = (slots.next(), slots.next());
    let keep_cptr = cnode.absolute_cptr(keep.cptr());
    let spare_cptr = cnode.absolute_cptr(spare.cptr());
    let mut at = desc.paddr();
    while at != paddr {
        let size_bits = (paddr - at).ilog2() as usize;
        device_ut
            .untyped_retype(&sel4::ObjectBlueprint::Untyped { size_bits }, &cnode.absolute_cptr_for_self(), spare.index(), 1)
            .expect("the device memory before the window is set aside");
        keep_cptr.delete().expect("the previous set-aside slot is freed");
        keep_cptr.move_(&spare_cptr).expect("the set-aside memory is kept");
        at += 1 << size_bits;
    }
    let frame_slot = slots.next().downcast::<sel4::cap_type::Granule>();
    device_ut
        .untyped_retype(&sel4::cap_type::Granule::object_blueprint(), &cnode.absolute_cptr_for_self(), frame_slot.index(), 1)
        .expect("the window's frame is made");
    let vaddr = ptr::addr_of!(WINDOW_PLACEHOLDERS) as usize + index * GRANULE_SIZE;
    user_image_frame(bootinfo, vaddr).cap().frame_unmap().expect("the placeholder page is unmapped");
    frame_slot
        .cap()
        .frame_map(sel4::init_thread::slot::VSPACE.cap(), vaddr, sel4::CapRights::read_write(), sel4::VmAttributes::default())
        .expect("the window is mapped");
    vaddr
}

fn user_image_frame(bootinfo: &sel4::BootInfoPtr, addr: usize) -> sel4::init_thread::Slot<sel4::cap_type::Granule> {
    unsafe extern "C" {
        static __executable_start: usize;
    }
    let image_start = ptr::addr_of!(__executable_start) as usize;
    bootinfo.user_image_frames().index(addr / GRANULE_SIZE - image_start / GRANULE_SIZE)
}

fn counter() -> (u64, u64) {
    let (ticks, frequency): (u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, cntvct_el0", out(reg) ticks);
        core::arch::asm!("mrs {}, cntfrq_el0", out(reg) frequency);
    }
    (ticks, frequency)
}

/// Milliseconds since boot from the generic timer: QEMU's virt board carries no real-time clock this host reads.
pub fn clock_millis() -> i64 {
    let (ticks, frequency) = counter();
    (ticks / (frequency / 1000).max(1)) as i64
}

/// The counter at boot: distinct per boot, but not cryptographic entropy (the board has no RNG this host reads).
pub fn counter_seed() -> u64 {
    counter().0
}
