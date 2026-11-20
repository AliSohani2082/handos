//! Loads one Vulkan driver instead of all of them.
//!
//! Spike 2: with every installed driver loaded, the overlay's RSS went from 24.7 to 69 MiB, which
//! fails the 40 MB budget. Pointing `VK_DRIVER_FILES` at the integrated GPU's driver fixes that.
#![allow(unsafe_code)]

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

pub const ICD_DIRS: [&str; 2] = ["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"];
const PRESET_VARS: [&str; 2] = ["VK_DRIVER_FILES", "VK_ICD_FILENAMES"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Driver {
    /// `VK_DRIVER_FILES` or `VK_ICD_FILENAMES` was already set by the user; left alone.
    Preset { var: &'static str, value: OsString },
    /// `overlay.vulkan_driver` from the settings.
    Configured(PathBuf),
    /// The driver for the Intel (else AMD) GPU present in the machine.
    Detected(PathBuf),
    /// No single choice: the Vulkan loader loads every driver (and RSS goes up).
    Undecided { candidates: Vec<PathBuf> },
}

impl Driver {
    /// The file to put in `VK_DRIVER_FILES`, if this choice sets one.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Configured(path) | Self::Detected(path) => Some(path),
            Self::Preset { .. } | Self::Undecided { .. } => None,
        }
    }
}

const INTEL: u16 = 0x8086;
const AMD: u16 = 0x1002;
/// Integrated GPUs first: the overlay needs little, and they cost the least power.
const PREFERRED: [(u16, &[&str]); 2] = [(INTEL, &["intel_icd"]), (AMD, &["radeon_icd", "amd_icd"])];

/// The decision, without touching the environment. `vendors` are the PCI vendor ids of the
/// GPUs present: a driver is only pinned when its GPU is in the machine, because Mesa often
/// installs `intel_icd.json` and `radeon_icd.json` on machines with neither.
pub fn choose(
    preset: Option<(&'static str, OsString)>,
    configured: Option<&Path>,
    dirs: &[&Path],
    vendors: &[u16],
) -> Driver {
    if let Some((var, value)) = preset {
        return Driver::Preset { var, value };
    }
    if let Some(path) = configured {
        return Driver::Configured(path.to_owned());
    }
    let mut candidates: Vec<PathBuf> = dirs
        .iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let name = file_name(path);
            path.extension().is_some_and(|ext| ext == "json")
                // A 64-bit daemon cannot use a 32-bit driver (Arch's lib32-* packages).
                && !(cfg!(target_pointer_width = "64") && (name.contains("i686") || name.contains("i386")))
        })
        .collect();
    candidates.sort();
    for (vendor, prefixes) in PREFERRED {
        if !vendors.contains(&vendor) {
            continue;
        }
        let matching: Vec<&PathBuf> = candidates
            .iter()
            .filter(|path| {
                let name = file_name(path);
                prefixes.iter().any(|prefix| name.starts_with(prefix))
            })
            .collect();
        if let [one] = matching.as_slice() {
            return Driver::Detected((*one).clone());
        }
    }
    // NVIDIA-only hosts (and anything unrecognised) are supported: the loader picks among all
    // drivers, which works but loads every one of them.
    Driver::Undecided { candidates }
}

/// PCI vendor ids of the GPUs the kernel knows (`/sys/class/drm/cardN/device/vendor`).
pub fn gpu_vendors() -> Vec<u16> {
    let mut vendors: Vec<u16> = fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix("card")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .filter_map(|entry| fs::read_to_string(entry.path().join("device/vendor")).ok())
        .filter_map(|text| u16::from_str_radix(text.trim().trim_start_matches("0x"), 16).ok())
        .collect();
    vendors.sort_unstable();
    vendors.dedup();
    vendors
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Chooses the driver and sets `VK_DRIVER_FILES`, then logs the choice.
///
/// # Safety
///
/// Changes the process environment, so no other thread may exist yet (`std::env::set_var` is
/// unsound while another thread reads the environment). Call it first in `main`.
pub unsafe fn select_driver(configured: Option<&Path>) -> Driver {
    let preset = PRESET_VARS
        .iter()
        .find_map(|&var| std::env::var_os(var).map(|value| (var, value)));
    let dirs: Vec<&Path> = ICD_DIRS.iter().map(Path::new).collect();
    let driver = choose(preset, configured, &dirs, &gpu_vendors());
    if let Some(path) = driver.path() {
        // SAFETY: the caller guarantees this runs before any other thread exists.
        unsafe { std::env::set_var("VK_DRIVER_FILES", path) };
    }
    match &driver {
        Driver::Preset { var, value } => {
            tracing::info!(var, value = %value.to_string_lossy(), "vulkan driver preset, left alone");
        }
        Driver::Configured(path) => {
            tracing::info!(path = %path.display(), "vulkan driver from settings");
        }
        Driver::Detected(path) => tracing::info!(path = %path.display(), "vulkan driver"),
        Driver::Undecided { candidates } => tracing::warn!(
            ?candidates,
            "no Intel or AMD GPU with a matching driver: every Vulkan driver loads, which costs memory; set overlay.vulkan_driver to pin one"
        ),
    }
    driver
}

#[cfg(test)]
mod tests {
    use super::*;

    const NVIDIA: u16 = 0x10de;

    fn icd_dir(names: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for name in names {
            fs::write(dir.path().join(name), "{}").unwrap();
        }
        dir
    }

    #[test]
    fn this_laptop_picks_intel_over_hasvk_and_nvidia() {
        let dir = icd_dir(&["intel_icd.json", "intel_hasvk_icd.json", "nvidia_icd.json"]);
        assert_eq!(
            choose(None, None, &[dir.path()], &[INTEL, NVIDIA]),
            Driver::Detected(dir.path().join("intel_icd.json"))
        );
    }

    #[test]
    fn a_driver_is_only_pinned_when_its_gpu_is_present() {
        let dir = icd_dir(&["intel_icd.json", "radeon_icd.json", "nvidia_icd.json"]);
        assert_eq!(
            choose(None, None, &[dir.path()], &[AMD]),
            Driver::Detected(dir.path().join("radeon_icd.json"))
        );
        // NVIDIA only: Mesa's Intel driver is installed but has no device; let the loader pick.
        assert!(matches!(
            choose(None, None, &[dir.path()], &[NVIDIA]),
            Driver::Undecided { candidates } if candidates.len() == 3
        ));
    }

    #[test]
    fn thirty_two_bit_drivers_are_ignored_on_a_64_bit_build() {
        let dir = icd_dir(&["intel_icd.x86_64.json", "intel_icd.i686.json"]);
        assert_eq!(
            choose(None, None, &[dir.path()], &[INTEL]),
            Driver::Detected(dir.path().join("intel_icd.x86_64.json"))
        );
    }

    #[test]
    fn no_drivers_or_no_gpus_are_left_to_the_loader() {
        let empty = icd_dir(&[]);
        assert_eq!(
            choose(
                None,
                None,
                &[empty.path(), Path::new("/nonexistent")],
                &[INTEL]
            ),
            Driver::Undecided { candidates: vec![] }
        );
        let dir = icd_dir(&["intel_icd.json"]);
        assert!(matches!(
            choose(None, None, &[dir.path()], &[]),
            Driver::Undecided { .. }
        ));
    }

    #[test]
    fn the_environment_wins_then_the_settings() {
        let dir = icd_dir(&["intel_icd.json"]);
        let preset = Some(("VK_ICD_FILENAMES", OsString::from("/x.json")));
        assert_eq!(
            choose(preset, Some(Path::new("/y.json")), &[dir.path()], &[INTEL]),
            Driver::Preset {
                var: "VK_ICD_FILENAMES",
                value: "/x.json".into()
            }
        );
        assert_eq!(
            choose(None, Some(Path::new("/y.json")), &[dir.path()], &[INTEL]),
            Driver::Configured("/y.json".into())
        );
    }

    #[test]
    fn this_machine_reports_its_gpus() {
        // Every Linux machine with a GPU exposes at least one DRM card; CI containers may not.
        let vendors = gpu_vendors();
        assert!(vendors.windows(2).all(|w| w[0] < w[1]), "{vendors:?}");
    }
}
