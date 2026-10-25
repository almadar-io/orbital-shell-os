//! The root task of an Orbital OS image. seL4 hands it every capability at boot; it runs the orbital-core kernel on
//! the embedded program and answers the kernel's asks: device operators against the program's `dev` grants (a mapped
//! register window, a bound interrupt line), everything else through the kernel's blocking host. It decides nothing.

#![no_std]
#![no_main]

extern crate alloc;

mod devices;

use alloc::string::ToString;
use alloc::sync::Arc;
use orbital_core_base::effect_types::RuntimeEnvironment;
use orbital_core_base::protocols::ProtocolSide;
use orbital_core_base::schema::types::OrbitalSchema;
use orbital_core_base::value::Value;
use orbital_core_runtime::resumable::{drive_blocking, BlockingHost, IoAnswer, IoRequest};
use orbital_core_runtime::{InMemoryEntityCache, KernelConfig, RuntimeKernel};
use sel4_root_task::{root_task, Never};

const PROGRAM: &str = include_str!("../program.orb");
const LANGUAGE: &str = include_str!("../language.json");

/// The frame a host start mounts its traits on (`orb run` names none).
const RUN_ENTITY: &str = "default";

#[root_task(stack_size = 4 * 1024 * 1024, heap_size = 128 * 1024 * 1024)]
fn main(bootinfo: &sel4::BootInfoPtr) -> sel4::Result<Never> {
    orbital_core_base::time_util::install_clock(devices::clock_millis);
    orbital_core_base::identity::install_process_seed(devices::counter_seed());
    orbital_core_eval::language_loader::install_language_bundle(LANGUAGE).expect("the embedded language bundle installs");

    let schema: OrbitalSchema = serde_json::from_str(PROGRAM).expect("the embedded program is a resolved schema");
    let cache = Arc::new(InMemoryEntityCache::new());
    let config = KernelConfig {
        environment: RuntimeEnvironment::Server,
        runtime_rows_in_kernel: true,
        ..KernelConfig::default()
    };
    let kernel = RuntimeKernel::new(RuntimeEnvironment::Server, cache.clone(), config);
    kernel.register_schema(schema).expect("the program registers");

    let devices = devices::Devices::claim(bootinfo, &kernel.os_grants());
    let rows = BlockingHost { source: cache.as_ref(), service: None, substrate: None };
    let answer = |request: &IoRequest| -> IoAnswer {
        match request {
            IoRequest::Os { .. } => devices.answer(&kernel.os_grants(), request),
            other => rows.answer(other),
        }
    };

    let seeds = kernel.boot_mounts(ProtocolSide::Start);
    let started = kernel.start_dispatch_mount(&seeds, Some(RUN_ENTITY), Value::Null).expect("the boot mounts start");
    drive_blocking(&kernel, started, &answer).expect("the boot mounts run");

    loop {
        for (site, key, payload) in devices.wait_for_interrupts() {
            let Some((event, payload)) = orbital_core_runtime::resumable::completion_event(&site, &key, Ok(payload)) else {
                continue;
            };
            let entity = site.entity_id.to_string();
            let started = kernel
                .start_dispatch(&site.trait_name, Some(&entity), &event, payload, None, None)
                .expect("an interrupt completion starts");
            drive_blocking(&kernel, started, &answer).expect("an interrupt completion runs");
        }
    }
}
