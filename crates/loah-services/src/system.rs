//! What the app needs to know about the machine it runs on.

use sysinfo::{MemoryRefreshKind, System};

const GIB: u64 = 1 << 30;

/// The installed RAM in whole GiB, rounded up; 0 when the OS does not say.
///
/// Rounded up because the OS reports the memory it can use, a little less than what is installed (a "16 GB" PC
/// shows 15.4 GiB), and a floor would push it under the 16 GB line of the model recommendation.
pub fn total_ram_gb() -> u64 {
    let mut system = System::new();
    system.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram());
    gib_from_bytes(system.total_memory())
}

/// Whole GiB in a number of bytes, rounded up.
fn gib_from_bytes(bytes: u64) -> u64 {
    bytes.div_ceil(GIB)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_gib_stay_as_they_are() {
        assert_eq!(gib_from_bytes(0), 0);
        assert_eq!(gib_from_bytes(8 * GIB), 8);
        assert_eq!(gib_from_bytes(16 * GIB), 16);
    }

    #[test]
    fn memory_the_os_keeps_back_is_rounded_up() {
        // 15.4 GiB: a "16 GB" PC with some memory reserved for the graphics chip.
        assert_eq!(gib_from_bytes(16 * GIB - 614 * 1024 * 1024), 16);
        assert_eq!(gib_from_bytes(8 * GIB - 1), 8);
        assert_eq!(gib_from_bytes(1), 1);
    }

    #[test]
    fn this_machine_has_some_ram() {
        assert!(total_ram_gb() >= 1);
    }
}
