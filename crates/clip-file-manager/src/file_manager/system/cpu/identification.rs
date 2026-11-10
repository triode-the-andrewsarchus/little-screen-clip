//! CPU-local identification in the disposable metadata helper, never via a device.
use super::super::{Document, model::Fact};
use raw_cpuid::{CpuId, CpuIdReader};
use std::{io, path::Path};

fn decoded<R: CpuIdReader>(reader: &CpuId<R>) -> Vec<(&'static str, String)> {
    let mut fields = Vec::new();
    if let Some(vendor) = reader.get_vendor_info() {
        fields.push(("CPU vendor", vendor.as_str().into()));
    }
    if let Some(brand) = reader.get_processor_brand_string() {
        fields.push(("CPU model", brand.as_str().into()));
    }
    if let Some(info) = reader.get_feature_info() {
        fields.push((
            "CPU family/model/stepping",
            format!(
                "{} / {} / {}",
                info.family_id(),
                info.model_id(),
                info.stepping_id()
            ),
        ));
        fields.push(("CPUID base capabilities", format!("{info:?}")));
    }
    if let Some(info) = reader.get_extended_feature_info() {
        fields.push(("CPUID structured capabilities", format!("{info:?}")));
    }
    if let Some(info) = reader.get_extended_processor_and_feature_identifiers() {
        fields.push(("CPUID extended capabilities", format!("{info:?}")));
    }
    fields
}

struct Affinity {
    original: libc::cpu_set_t,
    restored: bool,
}
impl Affinity {
    fn restore(&mut self) -> io::Result<()> {
        // SAFETY: the initialized saved mask remains live for this call; pid 0
        // changes only this helper thread's scheduling affinity.
        if unsafe {
            libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &self.original)
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        self.restored = true;
        Ok(())
    }
}
impl Drop for Affinity {
    fn drop(&mut self) {
        if !self.restored {
            let _ = self.restore();
        }
    }
}

fn pinned(id: u32) -> io::Result<Vec<(&'static str, String)>> {
    if id as usize >= libc::CPU_SETSIZE as usize {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "CPU exceeds bounded affinity-mask capacity",
        ));
    }
    // SAFETY: ARCH_GET_CPUID ignores addr and reports the current thread's flag.
    // Do not execute CPUID if the kernel cannot confirm it is enabled.
    if unsafe { libc::syscall(libc::SYS_arch_prctl, 0x1011, 0) } != 1 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "CPUID execution is disabled or unverified",
        ));
    }
    // SAFETY: zero is a valid empty CPU mask; getaffinity writes the owned mask.
    let mut original: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut original) }
        != 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut guard = Affinity {
        original,
        restored: false,
    };
    // SAFETY: the CPU index was checked against CPU_SETSIZE and the mask is live.
    let mut selected: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    unsafe { libc::CPU_SET(id as usize, &mut selected) };
    if unsafe { libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &selected) } != 0
    {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: sched_getcpu has no pointer arguments or side effects.
    if unsafe { libc::sched_getcpu() } != id as i32 {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "CPU placement could not be verified",
        ));
    }
    let result = decoded(&CpuId::new());
    if unsafe { libc::sched_getcpu() } != id as i32 {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "CPU changed during CPUID observation",
        ));
    }
    guard.restore()?;
    Ok(result)
}

pub(super) fn collect(document: &mut Document, cpu: &Path, id: u32) {
    use little_clip_core::kernel_interface::{self, Filesystem, Kind};
    // Ordinary synthetic fixtures and non-sysfs lookalikes must not probe hardware.
    if !kernel_interface::classify(cpu).is_ok_and(|kind| {
        kind == Kind::Kernel {
            filesystem: Filesystem::Sysfs,
        }
    }) {
        return;
    }
    match pinned(id) {
        Ok(fields) => {
            for (label, value) in fields {
                let mut fact = Fact::text(label, value, cpu.into());
                fact.origin = format!("CPUID instruction on pinned logical CPU {id}");
                document.facts.push(fact);
            }
            document.facts.push(Fact::text("CPU feature interpretation",
                "Advertised CPUID capabilities; enabled OS state and usable instructions can differ. Topology/cache sharing remains kernel-reported. No CPUID or MSR device was opened.", cpu.into()));
        }
        Err(error) => document
            .facts
            .push(Fact::failure("CPU identification", cpu.into(), error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raw_cpuid::CpuIdResult;
    #[test]
    fn synthetic_cpuid_reports_vendor_model_and_advertised_capabilities() {
        let reader = CpuId::with_cpuid_reader(|leaf, _| {
            let mut result = CpuIdResult {
                eax: 0,
                ebx: 0,
                ecx: 0,
                edx: 0,
            };
            match leaf {
                0 => {
                    result.eax = 7;
                    result.ebx = u32::from_le_bytes(*b"Genu");
                    result.edx = u32::from_le_bytes(*b"ineI");
                    result.ecx = u32::from_le_bytes(*b"ntel");
                }
                1 => {
                    result.eax = 0x306a9;
                    result.edx = 1 << 26;
                }
                7 => result.ebx = 1 << 5,
                0x80000000 => result.eax = 0x80000004,
                0x80000002 => {
                    result.eax = u32::from_le_bytes(*b"Test");
                    result.ebx = u32::from_le_bytes(*b" CPU");
                }
                _ => {}
            }
            result
        });
        let fields = decoded(&reader);
        assert!(fields.contains(&("CPU vendor", "GenuineIntel".into())));
        assert!(fields.contains(&("CPU model", "Test CPU".into())));
        assert!(fields.iter().any(|(_, value)| value.contains("SSE2")));
        assert!(fields.iter().any(|(_, value)| value.contains("AVX2")));
    }
    #[test]
    fn unsupported_leaves_and_excessive_cpu_ids_have_explicit_fallbacks() {
        let reader = CpuId::with_cpuid_reader(|_, _| CpuIdResult {
            eax: 0,
            ebx: 0,
            ecx: 0,
            edx: 0,
        });
        assert!(
            !decoded(&reader)
                .iter()
                .any(|(label, _)| *label == "CPU model")
        );
        assert_eq!(
            pinned(u32::MAX).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }
}
