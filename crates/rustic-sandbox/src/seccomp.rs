//! The Linux renderer system-call filter (ISO-03, ADR 0058).
//!
//! [`renderer_filter`] builds a classic-BPF program in the
//! `struct sock_filter` layout that bubblewrap's `--seccomp` installs in
//! the renderer just before it executes it. It is an **allowlist**: the
//! renderer may read and write the descriptors it has, map memory, use
//! threads, signals, clocks and randomness, and execute itself once;
//! every other system call fails with `ENOSYS` (C libraries then take
//! their fallbacks), so it can open no sockets, create no namespaces,
//! trace no process and reach no rarely used kernel interface. `clone` is
//! allowed without `CLONE_NEW*` flags, `ioctl` without terminal input
//! injection. bubblewrap's PID-1 init shares the filter, so it may wait
//! for its child. Other ABIs are killed, so 32-bit or x32 system calls cannot
//! bypass the table. The list was measured by tracing the renderer
//! through the CLI test suites. Building the program is safe Rust; it is
//! checked here by an interpreter of the same instructions.

/// One BPF instruction (`struct sock_filter`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Instruction {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

const LD_W_ABS: u16 = 0x20;
const JEQ: u16 = 0x15;
const JGE: u16 = 0x35;
const JSET: u16 = 0x45;
const RET: u16 = 0x06;

/// `SECCOMP_RET_ALLOW`.
pub const ALLOW: u32 = 0x7fff_0000;
/// `SECCOMP_RET_KILL_PROCESS`.
pub const KILL_PROCESS: u32 = 0x8000_0000;
/// `SECCOMP_RET_ERRNO` with `errno`.
pub const fn errno(errno: u16) -> u32 {
    0x0005_0000 | errno as u32
}
const EPERM: u16 = 1;
const ENOSYS: u16 = 38;

/// Offsets in `struct seccomp_data`; argument offsets are the low 32 bits
/// on the little-endian architectures supported here.
const NR: u32 = 0;
const ARCH: u32 = 4;
const fn argument_low(index: u32) -> u32 {
    16 + 8 * index
}

/// Every `CLONE_NEW*` flag: a confined renderer creates no namespaces.
const CLONE_NEW_NAMESPACES: u32 = 0x0002_0000 // NEWNS
    | 0x0200_0000 // NEWCGROUP
    | 0x0400_0000 // NEWUTS
    | 0x0800_0000 // NEWIPC
    | 0x1000_0000 // NEWUSER
    | 0x2000_0000 // NEWPID
    | 0x4000_0000 // NEWNET
    | 0x0000_0080; // NEWTIME
/// `TIOCSTI` and `TIOCLINUX` inject input into a terminal.
const TERMINAL_INJECTION: [u32; 2] = [0x5412, 0x541C];

/// An architecture the filter knows the system-call table of.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    /// x86-64 (`AUDIT_ARCH_X86_64`).
    X86_64,
    /// 64-bit ARM (`AUDIT_ARCH_AARCH64`).
    Aarch64,
}

impl Arch {
    /// The architecture this crate was compiled for, if supported.
    pub const fn current() -> Option<Self> {
        if cfg!(target_arch = "x86_64") {
            Some(Self::X86_64)
        } else if cfg!(target_arch = "aarch64") {
            Some(Self::Aarch64)
        } else {
            None
        }
    }

    const fn audit(self) -> u32 {
        match self {
            Self::X86_64 => 0xC000_003E,
            Self::Aarch64 => 0xC000_00B7,
        }
    }
}

/// The allowed system calls as `(name, x86-64 number, aarch64 number)`;
/// `None` where the call does not exist. Numbers are from the kernel's
/// `unistd_64.h` and the generic `unistd.h`. `execve` is needed once:
/// bubblewrap installs the filter before executing the renderer.
const ALLOWED: &[(&str, Option<u32>, Option<u32>)] = &[
    ("read", Some(0), Some(63)),
    ("write", Some(1), Some(64)),
    ("readv", Some(19), Some(65)),
    ("writev", Some(20), Some(66)),
    ("pread64", Some(17), Some(67)),
    ("pwrite64", Some(18), Some(68)),
    ("preadv", Some(295), Some(69)),
    ("pwritev", Some(296), Some(70)),
    ("lseek", Some(8), Some(62)),
    ("close", Some(3), Some(57)),
    ("close_range", Some(436), Some(436)),
    ("open", Some(2), None),
    ("openat", Some(257), Some(56)),
    ("stat", Some(4), None),
    ("fstat", Some(5), Some(80)),
    ("lstat", Some(6), None),
    ("newfstatat", Some(262), Some(79)),
    ("statx", Some(332), Some(291)),
    ("getdents64", Some(217), Some(61)),
    ("access", Some(21), None),
    ("faccessat", Some(269), Some(48)),
    ("faccessat2", Some(439), Some(439)),
    ("readlink", Some(89), None),
    ("readlinkat", Some(267), Some(78)),
    ("getcwd", Some(79), Some(17)),
    ("fcntl", Some(72), Some(25)),
    ("dup", Some(32), Some(23)),
    ("dup2", Some(33), None),
    ("dup3", Some(292), Some(24)),
    ("pipe2", Some(293), Some(59)),
    ("poll", Some(7), None),
    ("ppoll", Some(271), Some(73)),
    ("pselect6", Some(270), Some(72)),
    ("mmap", Some(9), Some(222)),
    ("munmap", Some(11), Some(215)),
    ("mprotect", Some(10), Some(226)),
    ("mremap", Some(25), Some(216)),
    ("madvise", Some(28), Some(233)),
    ("brk", Some(12), Some(214)),
    ("rt_sigaction", Some(13), Some(134)),
    ("rt_sigprocmask", Some(14), Some(135)),
    ("rt_sigreturn", Some(15), Some(139)),
    ("sigaltstack", Some(131), Some(132)),
    ("tgkill", Some(234), Some(131)),
    ("tkill", Some(200), Some(130)),
    ("getpid", Some(39), Some(172)),
    ("gettid", Some(186), Some(178)),
    ("getppid", Some(110), Some(173)),
    ("getuid", Some(102), Some(174)),
    ("geteuid", Some(107), Some(175)),
    ("getgid", Some(104), Some(176)),
    ("getegid", Some(108), Some(177)),
    ("getresuid", Some(118), Some(148)),
    ("getresgid", Some(120), Some(150)),
    ("set_tid_address", Some(218), Some(96)),
    ("set_robust_list", Some(273), Some(99)),
    ("rseq", Some(334), Some(293)),
    ("futex", Some(202), Some(98)),
    ("sched_yield", Some(24), Some(124)),
    ("sched_getaffinity", Some(204), Some(123)),
    ("clock_gettime", Some(228), Some(113)),
    ("clock_getres", Some(229), Some(114)),
    ("clock_nanosleep", Some(230), Some(115)),
    ("nanosleep", Some(35), Some(101)),
    ("gettimeofday", Some(96), Some(169)),
    ("time", Some(201), None),
    ("getrandom", Some(318), Some(278)),
    ("prlimit64", Some(302), Some(261)),
    ("getrlimit", Some(97), Some(163)),
    ("arch_prctl", Some(158), None),
    ("exit", Some(60), Some(93)),
    ("exit_group", Some(231), Some(94)),
    ("execve", Some(59), Some(221)),
    // bubblewrap's PID-1 init in the renderer's namespace runs under the
    // same filter and reaps the renderer.
    ("wait4", Some(61), Some(260)),
    ("waitid", Some(247), Some(95)),
    ("uname", Some(63), Some(160)),
    ("sysinfo", Some(99), Some(179)),
    ("membarrier", Some(324), Some(283)),
];

/// `(clone, ioctl)` numbers.
const fn special(arch: Arch) -> (u32, u32) {
    match arch {
        Arch::X86_64 => (56, 16),
        Arch::Aarch64 => (220, 29),
    }
}

fn allowed_numbers(arch: Arch) -> impl Iterator<Item = u32> {
    ALLOWED
        .iter()
        .filter_map(move |&(_, x86_64, aarch64)| match arch {
            Arch::X86_64 => x86_64,
            Arch::Aarch64 => aarch64,
        })
}

/// The renderer filter for `arch`.
pub fn renderer_filter(arch: Arch) -> Vec<Instruction> {
    let op = |code, jt, jf, k| Instruction { code, jt, jf, k };
    let mut program = vec![
        op(LD_W_ABS, 0, 0, ARCH),
        op(JEQ, 1, 0, arch.audit()),
        op(RET, 0, 0, KILL_PROCESS),
        op(LD_W_ABS, 0, 0, NR),
    ];
    if arch == Arch::X86_64 {
        // x32 system calls set bit 30 and reach a second table.
        program.push(op(JGE, 0, 1, 0x4000_0000));
        program.push(op(RET, 0, 0, KILL_PROCESS));
    }
    let (clone, ioctl) = special(arch);
    // clone: threads and processes, but no new namespaces. (clone3 is not
    // allowed: its flags are in memory a filter cannot read, so it fails
    // with ENOSYS and C libraries fall back to clone.)
    program.push(op(JEQ, 0, 4, clone));
    program.push(op(LD_W_ABS, 0, 0, argument_low(0)));
    program.push(op(JSET, 0, 1, CLONE_NEW_NAMESPACES));
    program.push(op(RET, 0, 0, errno(EPERM)));
    program.push(op(RET, 0, 0, ALLOW));
    // ioctl: no terminal input injection.
    program.push(op(JEQ, 0, 5, ioctl));
    program.push(op(LD_W_ABS, 0, 0, argument_low(1)));
    program.push(op(JEQ, 2, 0, TERMINAL_INJECTION[0]));
    program.push(op(JEQ, 1, 0, TERMINAL_INJECTION[1]));
    program.push(op(RET, 0, 0, ALLOW));
    program.push(op(RET, 0, 0, errno(EPERM)));
    // The allowlist jumps to the final ALLOW; everything else is ENOSYS.
    let numbers: Vec<u32> = allowed_numbers(arch).collect();
    for (index, &number) in numbers.iter().enumerate() {
        // Distance to the final instruction: the remaining comparisons
        // and the ENOSYS return. The list is far below 255 entries.
        let distance = u8::try_from(numbers.len() - index).expect("allowlist below 255 entries");
        program.push(op(JEQ, distance, 0, number));
    }
    program.push(op(RET, 0, 0, errno(ENOSYS)));
    program.push(op(RET, 0, 0, ALLOW));
    program
}

/// The program as the bytes `bwrap --seccomp` reads (native byte order).
pub fn to_bytes(program: &[Instruction]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(program.len() * 8);
    for instruction in program {
        bytes.extend_from_slice(&instruction.code.to_ne_bytes());
        bytes.push(instruction.jt);
        bytes.push(instruction.jf);
        bytes.extend_from_slice(&instruction.k.to_ne_bytes());
    }
    bytes
}

/// Run `program` on a system call, as the kernel would: the reference
/// used by the tests and the fuzz-free checks of [`renderer_filter`].
/// `None` for a malformed program (unknown opcode, a jump or load out of
/// range, or no return).
pub fn evaluate(program: &[Instruction], arch: u32, nr: u32, args: [u64; 6]) -> Option<u32> {
    let mut data = Vec::with_capacity(64);
    data.extend_from_slice(&nr.to_le_bytes());
    data.extend_from_slice(&arch.to_le_bytes());
    data.extend_from_slice(&0u64.to_le_bytes());
    for argument in args {
        data.extend_from_slice(&argument.to_le_bytes());
    }
    let mut accumulator = 0u32;
    let mut pc = 0usize;
    // Classic BPF only jumps forward, so this ends within `len` steps.
    while let Some(instruction) = program.get(pc) {
        let jump = |taken: bool| {
            pc + 1
                + usize::from(if taken {
                    instruction.jt
                } else {
                    instruction.jf
                })
        };
        pc = match instruction.code {
            LD_W_ABS => {
                let at = usize::try_from(instruction.k).ok()?;
                let word = data.get(at..at.checked_add(4)?)?;
                accumulator = u32::from_le_bytes(word.try_into().ok()?);
                pc + 1
            }
            JEQ => jump(accumulator == instruction.k),
            JGE => jump(accumulator >= instruction.k),
            JSET => jump(accumulator & instruction.k != 0),
            RET => return Some(instruction.k),
            _ => return None,
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARCHES: [Arch; 2] = [Arch::X86_64, Arch::Aarch64];

    fn decide(arch: Arch, nr: u32, args: [u64; 6]) -> u32 {
        evaluate(&renderer_filter(arch), arch.audit(), nr, args).expect("well-formed")
    }

    /// Calls that must never pass, as `(name, x86-64, aarch64)`: tracing,
    /// kernel modules, mounting and namespaces, BPF and perf, keyrings,
    /// io_uring, sockets, host and clock changes.
    const DANGEROUS: &[(&str, Option<u32>, Option<u32>)] = &[
        ("ptrace", Some(101), Some(117)),
        ("process_vm_readv", Some(310), Some(270)),
        ("process_vm_writev", Some(311), Some(271)),
        ("kexec_load", Some(246), Some(104)),
        ("init_module", Some(175), Some(105)),
        ("finit_module", Some(313), Some(273)),
        ("mount", Some(165), Some(40)),
        ("umount2", Some(166), Some(39)),
        ("pivot_root", Some(155), Some(41)),
        ("chroot", Some(161), Some(51)),
        ("open_tree", Some(428), Some(428)),
        ("unshare", Some(272), Some(97)),
        ("setns", Some(308), Some(268)),
        ("bpf", Some(321), Some(280)),
        ("perf_event_open", Some(298), Some(241)),
        ("userfaultfd", Some(323), Some(282)),
        ("keyctl", Some(250), Some(219)),
        ("add_key", Some(248), Some(217)),
        ("io_uring_setup", Some(425), Some(425)),
        ("personality", Some(135), Some(92)),
        ("socket", Some(41), Some(198)),
        ("socketpair", Some(53), Some(199)),
        ("connect", Some(42), Some(203)),
        ("reboot", Some(169), Some(142)),
        ("settimeofday", Some(164), Some(170)),
        ("sethostname", Some(170), Some(161)),
        ("open_by_handle_at", Some(304), Some(265)),
        ("syslog", Some(103), Some(116)),
        ("move_pages", Some(279), Some(239)),
        ("kcmp", Some(312), Some(272)),
        ("fork", Some(57), None),
        ("vfork", Some(58), None),
        ("clone3", Some(435), Some(435)),
    ];

    #[test]
    fn dangerous_calls_fail_and_measured_calls_pass() {
        for arch in ARCHES {
            for &(name, x86_64, aarch64) in DANGEROUS {
                let number = if arch == Arch::X86_64 {
                    x86_64
                } else {
                    aarch64
                };
                if let Some(number) = number {
                    assert_eq!(decide(arch, number, [0; 6]), errno(ENOSYS), "{name}");
                }
            }
            for number in allowed_numbers(arch) {
                assert_eq!(decide(arch, number, [0; 6]), ALLOW, "{number}");
            }
            // read, write, mmap, futex, openat, exit_group.
            let ordinary: &[u32] = match arch {
                Arch::X86_64 => &[0, 1, 9, 202, 257, 231],
                Arch::Aarch64 => &[63, 64, 222, 98, 56, 94],
            };
            for &number in ordinary {
                assert_eq!(decide(arch, number, [0; 6]), ALLOW, "{number}");
            }
        }
    }

    #[test]
    fn the_allowlist_has_no_duplicates_or_dangerous_calls() {
        for arch in ARCHES {
            let mut numbers: Vec<u32> = allowed_numbers(arch).collect();
            let count = numbers.len();
            numbers.sort_unstable();
            numbers.dedup();
            assert_eq!(numbers.len(), count, "{arch:?}");
            let (clone, ioctl) = special(arch);
            assert!(!numbers.contains(&clone) && !numbers.contains(&ioctl));
        }
        for &(name, ..) in DANGEROUS {
            assert!(
                ALLOWED.iter().all(|&(allowed, ..)| allowed != name),
                "{name}"
            );
        }
    }

    #[test]
    fn clone_creates_threads_but_no_namespaces() {
        for arch in ARCHES {
            let (clone, _) = special(arch);
            // CLONE_VM | CLONE_FS | CLONE_FILES | CLONE_SIGHAND | CLONE_THREAD.
            assert_eq!(decide(arch, clone, [0x0001_0f00, 0, 0, 0, 0, 0]), ALLOW);
            for flag in [0x1000_0000, 0x4000_0000, 0x0002_0000, 0x80] {
                assert_eq!(decide(arch, clone, [flag, 0, 0, 0, 0, 0]), errno(EPERM));
            }
        }
    }

    #[test]
    fn terminal_injection_ioctls_are_refused() {
        for arch in ARCHES {
            let (_, ioctl) = special(arch);
            for request in TERMINAL_INJECTION {
                assert_eq!(
                    decide(arch, ioctl, [1, u64::from(request), 0, 0, 0, 0]),
                    errno(EPERM)
                );
            }
            // TCGETS.
            assert_eq!(decide(arch, ioctl, [1, 0x5401, 0, 0, 0, 0]), ALLOW);
        }
    }

    #[test]
    fn other_abis_are_killed() {
        let program = renderer_filter(Arch::X86_64);
        // i386 (AUDIT_ARCH_I386) and x32 (bit 30).
        assert_eq!(
            evaluate(&program, 0x4000_0003, 1, [0; 6]),
            Some(KILL_PROCESS)
        );
        assert_eq!(
            evaluate(&program, Arch::X86_64.audit(), 0x4000_0001, [0; 6]),
            Some(KILL_PROCESS)
        );
        let program = renderer_filter(Arch::Aarch64);
        assert_eq!(
            evaluate(&program, Arch::X86_64.audit(), 63, [0; 6]),
            Some(KILL_PROCESS)
        );
    }

    #[test]
    fn programs_are_within_kernel_limits_and_serialize_eight_bytes_each() {
        for arch in ARCHES {
            let program = renderer_filter(arch);
            assert!(program.len() <= 4096);
            assert_eq!(to_bytes(&program).len(), program.len() * 8);
            // Every syscall number reaches a return: no fall-through.
            for nr in 0..600 {
                assert!(evaluate(&program, arch.audit(), nr, [0; 6]).is_some());
            }
        }
        let first = to_bytes(&renderer_filter(Arch::X86_64));
        assert_eq!(&first[..2], &LD_W_ABS.to_ne_bytes());
        assert_eq!(&first[4..8], &ARCH.to_ne_bytes());
    }
}
