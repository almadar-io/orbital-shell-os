//! Orbital OS shell: the program, the language it runs against and the seL4 root task that hosts the kernel, written
//! as one project. The root task (`templates/sel4-root-task`) embeds both files and answers the kernel's asks on seL4.

pub mod shell_program;
