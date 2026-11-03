// OpenRevo - Hardware Blast Shield & Safety Gatekeeper
// Protects host silicon, battery charging circuitry, and motherboard firmware from catastrophic or invalid writes.
//
// Transition policy (see config field `raw_gateway_enforce`):
// - Audit mode (default): writes to protected registers and raw mailbox calls are logged, then allowed.
// - Enforce mode: the same operations are rejected.

use crate::hal::vault::ec;

/// Returns the semantic name of a register that third-party raw access must not change silently.
fn protected_register_name(addr: u16) -> Option<&'static str> {
    let protected: [(u16, &'static str); 31] = [
        (ec::ADDR_PROJECT_ID, "PROJECT_ID"),
        (ec::ADDR_PLATFORM_REAL_ID, "PLATFORM_REAL_ID"),
        (ec::ADDR_BATTERY_PROTECTION_MODE, "BATTERY_PROTECTION_MODE"),
        (ec::ADDR_BATTERY_LIMIT_UP, "BATTERY_LIMIT_UP"),
        (ec::ADDR_BATTERY_LIMIT_DOWN, "BATTERY_LIMIT_DOWN"),
        (ec::ADDR_BATTERY_PLATFORM_OVERRIDE, "BATTERY_PLATFORM_OVERRIDE"),
        (ec::ADDR_BATTERY_GATE_STATUS, "BATTERY_GATE_STATUS"),
        (ec::ADDR_POWER_STATUS, "POWER_STATUS"),
        (ec::ADDR_PL1_LOW, "PL1_LOW"),
        (ec::ADDR_PL1_HIGH, "PL1_HIGH"),
        (ec::ADDR_PL2_LOW, "PL2_LOW"),
        (ec::ADDR_PL2_HIGH, "PL2_HIGH"),
        (ec::ADDR_TGP_OFFSET, "TGP_OFFSET"),
        (ec::ADDR_OFFICE_PL1, "OFFICE_PL1"),
        (ec::ADDR_OFFICE_PL2, "OFFICE_PL2"),
        (ec::ADDR_OFFICE_PL4, "OFFICE_PL4"),
        (ec::ADDR_OFFICE_TCC, "OFFICE_TCC"),
        (ec::ADDR_BALANCE_PL1, "BALANCE_PL1"),
        (ec::ADDR_BALANCE_PL2, "BALANCE_PL2"),
        (ec::ADDR_BALANCE_PL4, "BALANCE_PL4"),
        (ec::ADDR_BALANCE_TCC, "BALANCE_TCC"),
        (ec::ADDR_TURBO_PL1, "TURBO_PL1"),
        (ec::ADDR_TURBO_PL2, "TURBO_PL2"),
        (ec::ADDR_TURBO_PL4, "TURBO_PL4"),
        (ec::ADDR_TURBO_TCC, "TURBO_TCC"),
        (ec::ADDR_CUSTOM_MODE, "CUSTOM_MODE"),
        (ec::ADDR_FAN_RAMP_RATE, "FAN_RAMP_RATE"),
        (ec::ADDR_FAN_ISOLATED, "FAN_ISOLATED"),
        (ec::ADDR_FAN_TABLE_LATCH, "FAN_TABLE_LATCH"),
        (ec::ADDR_FAN_BOOST_CLAMP, "FAN_BOOST_CLAMP"),
        (ec::ADDR_AC_RECOVERY, "AC_RECOVERY"),
    ];
    protected
        .iter()
        .find(|(protected_addr, _)| *protected_addr == addr)
        .map(|(_, name)| *name)
}

/// Asserts that a target EC RAM address is within standard physical boundary limits.
pub fn validate_read_address(addr: u16) -> Result<(), String> {
    // Standard and extended EC address spaces span up to 0x0FFF (4096 bytes)
    if addr > 0x0FFF {
        return Err(format!("Address out of physical bounds: 0x{:04X} exceeds 0x0FFF", addr));
    }
    Ok(())
}

/// Pure write policy. `Ok(Some(name))` means the write is allowed but must be recorded by the audit log.
fn evaluate_write(addr: u16, val: u8, enforce: bool) -> Result<Option<&'static str>, String> {
    validate_read_address(addr)?;

    // Address 0x0000 is typically reserved for critical reset/boot vector locks
    if addr == 0x0000 && val == 0xFF {
        return Err("Operation blocked by Hardware Blast Shield: Reserved reset vector".to_string());
    }

    match protected_register_name(addr) {
        Some(name) if enforce => Err(format!(
            "Operation blocked by Hardware Blast Shield: protected register {}",
            name
        )),
        other => Ok(other),
    }
}

/// Asserts that a target write operation does not touch protected firmware flash or hardware fuse registers.
pub fn validate_write_operation(addr: u16, val: u8) -> Result<(), String> {
    let enforce = crate::system::config::load_config().raw_gateway_enforce;
    match evaluate_write(addr, val, enforce) {
        Ok(Some(name)) => {
            crate::log_warn!(
                "BLAST_SHIELD",
                "[AUDIT] Raw write to protected register {} (0x{:04X}) allowed during transition; it will be blocked once enforcement is enabled",
                name,
                addr
            );
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(e) => {
            crate::log_warn!("BLAST_SHIELD", "Blocked raw write to 0x{:04X}: {}", addr, e);
            Err(e)
        }
    }
}

/// Raw mailbox frames are opaque to the shield, so they cannot be checked per register.
pub fn validate_mailbox_invoke(frame_len: usize) -> Result<(), String> {
    if crate::system::config::load_config().raw_gateway_enforce {
        crate::log_warn!("BLAST_SHIELD", "Blocked raw mailbox frame ({} bytes)", frame_len);
        return Err("Operation blocked by Hardware Blast Shield: raw mailbox access is disabled".to_string());
    }
    crate::log_warn!(
        "BLAST_SHIELD",
        "[AUDIT] Raw mailbox frame ({} bytes) allowed during transition; it will be blocked once enforcement is enabled",
        frame_len
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_out_of_bounds_and_reset_vector() {
        assert!(evaluate_write(0x1000, 0, false).is_err());
        assert!(evaluate_write(0x0000, 0xFF, false).is_err());
    }

    #[test]
    fn audit_mode_allows_protected_write_and_reports_it() {
        assert_eq!(
            evaluate_write(ec::ADDR_BATTERY_LIMIT_UP, 80, false),
            Ok(Some("BATTERY_LIMIT_UP"))
        );
    }

    #[test]
    fn enforce_mode_blocks_protected_write() {
        assert!(evaluate_write(ec::ADDR_BATTERY_LIMIT_UP, 80, true).is_err());
    }

    #[test]
    fn unprotected_write_is_allowed_in_both_modes() {
        assert_eq!(evaluate_write(ec::ADDR_RGB_RED, 1, false), Ok(None));
        assert_eq!(evaluate_write(ec::ADDR_RGB_RED, 1, true), Ok(None));
    }
}
