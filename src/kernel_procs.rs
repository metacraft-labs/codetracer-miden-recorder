//! Kernel procedure detection and handling for Miden VM traces.
//!
//! In Miden transactions, the VM executes kernel procedures (system calls) that
//! handle account storage, note processing, asset management, and other
//! protocol-level operations. These procedures have no user-written source code
//! and should be marked as "system" in the trace output.
//!
//! This module provides:
//! - [`is_kernel_procedure`]: detect whether a procedure name belongs to the kernel
//! - [`KernelProcInfo`]: metadata about a detected kernel procedure
//! - [`KERNEL_PREFIXES`]: known kernel procedure namespace prefixes

/// Known kernel procedure namespace prefixes.
///
/// Kernel procedures in Miden follow a naming convention where they are
/// namespaced under well-known prefixes. This list covers the standard
/// Miden kernel modules.
pub const KERNEL_PREFIXES: &[&str] = &[
    "miden::kernel::",
    "kernel::",
    "miden::account::",
    "miden::note::",
    "miden::tx::",
    "miden::asset::",
    "miden::faucet::",
    "miden::sat::internal::",
    // The kernel prologue/epilogue procedures
    "#sys::",
    "#kernel::",
];

/// Information about a detected kernel procedure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KernelProcInfo {
    /// The full qualified name of the kernel procedure.
    pub name: String,
    /// A short display name (last segment after "::").
    pub short_name: String,
    /// Whether this is a prologue/epilogue procedure (transaction setup/teardown).
    pub is_prologue: bool,
}

/// Check whether a procedure name belongs to the Miden kernel.
///
/// Returns `true` if the name starts with any of the known kernel prefixes.
///
/// # Examples
///
/// ```
/// use codetracer_miden_recorder::kernel_procs::is_kernel_procedure;
///
/// assert!(is_kernel_procedure("miden::kernel::account_vault_add_asset"));
/// assert!(is_kernel_procedure("miden::tx::get_block_number"));
/// assert!(!is_kernel_procedure("my_contract::transfer"));
/// ```
pub fn is_kernel_procedure(name: &str) -> bool {
    KERNEL_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// Extract information about a kernel procedure from its name.
///
/// Returns `None` if the name is not a kernel procedure.
pub fn kernel_proc_info(name: &str) -> Option<KernelProcInfo> {
    if !is_kernel_procedure(name) {
        return None;
    }

    let short_name = name.rsplit("::").next().unwrap_or(name).to_string();

    let is_prologue = short_name.contains("prologue")
        || short_name.contains("epilogue")
        || name.contains("#sys::");

    Some(KernelProcInfo {
        name: name.to_string(),
        short_name,
        is_prologue,
    })
}

/// Categorize a procedure name for trace display.
///
/// Returns a display-friendly label:
/// - For kernel procedures: `"[kernel] <short_name>"`
/// - For user procedures: the name as-is
pub fn procedure_display_name(name: &str) -> String {
    if let Some(info) = kernel_proc_info(name) {
        if info.is_prologue {
            format!("[kernel:setup] {}", info.short_name)
        } else {
            format!("[kernel] {}", info.short_name)
        }
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_kernel_procedure_known_prefixes() {
        assert!(is_kernel_procedure("miden::kernel::account_vault_add_asset"));
        assert!(is_kernel_procedure("miden::kernel::get_account_id"));
        assert!(is_kernel_procedure("kernel::some_proc"));
        assert!(is_kernel_procedure("miden::account::get_id"));
        assert!(is_kernel_procedure("miden::note::get_inputs"));
        assert!(is_kernel_procedure("miden::tx::get_block_number"));
        assert!(is_kernel_procedure("miden::asset::build_fungible_asset"));
        assert!(is_kernel_procedure("miden::faucet::mint"));
        assert!(is_kernel_procedure("miden::sat::internal::something"));
        assert!(is_kernel_procedure("#sys::prologue"));
        assert!(is_kernel_procedure("#kernel::some_proc"));
    }

    #[test]
    fn test_is_kernel_procedure_user_procs() {
        assert!(!is_kernel_procedure("my_contract::transfer"));
        assert!(!is_kernel_procedure("compute"));
        assert!(!is_kernel_procedure("#exec::compute"));
        assert!(!is_kernel_procedure("std::math::u64::add"));
        assert!(!is_kernel_procedure(""));
    }

    #[test]
    fn test_kernel_proc_info_extraction() {
        let info = kernel_proc_info("miden::kernel::account_vault_add_asset").unwrap();
        assert_eq!(info.name, "miden::kernel::account_vault_add_asset");
        assert_eq!(info.short_name, "account_vault_add_asset");
        assert!(!info.is_prologue);

        let info = kernel_proc_info("#sys::prologue").unwrap();
        assert_eq!(info.short_name, "prologue");
        assert!(info.is_prologue);

        assert!(kernel_proc_info("my_contract::transfer").is_none());
    }

    #[test]
    fn test_procedure_display_name() {
        assert_eq!(
            procedure_display_name("miden::kernel::get_account_id"),
            "[kernel] get_account_id"
        );
        assert_eq!(
            procedure_display_name("#sys::prologue"),
            "[kernel:setup] prologue"
        );
        assert_eq!(
            procedure_display_name("my_contract::transfer"),
            "my_contract::transfer"
        );
    }
}
