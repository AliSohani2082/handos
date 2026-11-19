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
    /// The one non-NVIDIA driver installed, or the one Intel/AMD driver among several.
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

/// The decision, without touching the environment.
pub fn choose(
    preset: Option<(&'static str, OsString)>,
    configured: Option<&Path>,
    dirs: &[&Path],
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
                && !name.contains("nvidia")
                // A 64-bit daemon cannot use a 32-bit driver (Arch's lib32-* packages).
                && !(cfg!(target_pointer_width = "64") && (name.contains("i686") || name.contains("i386")))
        })
        .collect();
    candidates.sort();
    if let [only] = candidates.as_slice() {
        return Driver::Detected(only.clone());
    }
    let preferred: Vec<&PathBuf> = candidates
        .iter()
        .filter(|path| {
            let name = file_name(path);
            name.starts_with("intel_icd") || name.starts_with("radeon_icd")
        })
        .collect();
    match preferred.as_slice() {
        [one] => Driver::Detected((*one).clone()),
        _ => Driver::Undecided { candidates },
    }
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
    let driver = choose(preset, configured, &dirs);
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
            "several Vulkan drivers and none preferred: all load, which costs memory; set overlay.vulkan_driver"
        ),
    }
    driver
}

#[cfg(test)]
mod tests {
    use super::*;

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
            choose(None, None, &[dir.path()]),
            Driver::Detected(dir.path().join("intel_icd.json"))
        );
    }

    #[test]
    fn a_single_non_nvidia_driver_is_used_whatever_its_name() {
        let dir = icd_dir(&["lvp_icd.x86_64.json", "nvidia_icd.json", "readme.txt"]);
        assert_eq!(
            choose(None, None, &[dir.path()]),
            Driver::Detected(dir.path().join("lvp_icd.x86_64.json"))
        );
    }

    #[test]
    fn thirty_two_bit_drivers_are_ignored_on_a_64_bit_build() {
        let dir = icd_dir(&["intel_icd.x86_64.json", "intel_icd.i686.json"]);
        assert_eq!(
            choose(None, None, &[dir.path()]),
            Driver::Detected(dir.path().join("intel_icd.x86_64.json"))
        );
    }

    #[test]
    fn ambiguous_drivers_are_left_to_the_loader() {
        let dir = icd_dir(&["intel_icd.json", "radeon_icd.json"]);
        assert!(matches!(
            choose(None, None, &[dir.path()]),
            Driver::Undecided { candidates } if candidates.len() == 2
        ));
        let empty = icd_dir(&[]);
        assert_eq!(
            choose(None, None, &[empty.path(), Path::new("/nonexistent")]),
            Driver::Undecided { candidates: vec![] }
        );
    }

    #[test]
    fn the_environment_wins_then_the_settings() {
        let dir = icd_dir(&["intel_icd.json"]);
        let preset = Some(("VK_ICD_FILENAMES", OsString::from("/x.json")));
        assert_eq!(
            choose(preset, Some(Path::new("/y.json")), &[dir.path()]),
            Driver::Preset {
                var: "VK_ICD_FILENAMES",
                value: "/x.json".into()
            }
        );
        assert_eq!(
            choose(None, Some(Path::new("/y.json")), &[dir.path()]),
            Driver::Configured("/y.json".into())
        );
    }
}
