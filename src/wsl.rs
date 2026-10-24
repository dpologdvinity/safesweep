//! Windows Subsystem for Linux (WSL) support: detecting it, and explaining how to give disk space
//! back to Windows.
//!
//! On WSL 2 the Linux filesystem lives in a single virtual disk file (`ext4.vhdx`) on the Windows
//! side. Deleting files inside Linux frees space *inside* that file but does not make the file
//! smaller, so Windows' free space does not change until the disk is compacted from Windows.
//! This module only produces instructions. It never runs anything, and it never touches the
//! Windows side.

/// True if the text of `/proc/version` shows a WSL kernel (WSL kernels are built by Microsoft and
/// say so, for example `6.6.36.3-microsoft-standard-WSL2`).
///
/// `proc_version` is the contents of `/proc/version`.
pub fn is_wsl(proc_version: &str) -> bool {
    proc_version.to_lowercase().contains("microsoft")
}

/// The instructions for compacting the virtual disk of the WSL distribution `distro`, as text
/// to show the user. They are printed, never run: compacting needs administrator rights on
/// Windows and a full WSL shutdown.
///
/// The steps are: back up the distribution, shut WSL down, find the `ext4.vhdx` file through the
/// registry, then compact it with `Optimize-VHD` (or `diskpart` where Hyper-V is not available).
///
/// This deliberately does not suggest `wsl --manage --set-sparse`: WSL disabled sparse disks
/// after data-corruption reports, and a sparse disk cannot be compacted with `Optimize-VHD`.
pub fn hint(distro: &str) -> String {
    format!(
        r#"WSL: freed space stays inside ext4.vhdx until you compact it from Windows.
In PowerShell (as admin):
  1. Back up:   wsl --export {distro} {distro}-backup.tar
  2. Stop WSL:  wsl --shutdown
  3. Find the disk:
     (Get-ChildItem HKCU:\Software\Microsoft\Windows\CurrentVersion\Lxss | Get-ItemProperty | Where-Object DistributionName -eq '{distro}').BasePath
     The file is ext4.vhdx in that folder.
  4. Compact:   Optimize-VHD -Path <folder>\ext4.vhdx -Mode Full
     (no Hyper-V? diskpart: select vdisk file="<folder>\ext4.vhdx", attach vdisk readonly, compact vdisk, detach vdisk)
"#
    )
}

/// Unit tests for WSL detection and the shape of the instructions.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_wsl() {
        assert!(is_wsl("Linux version 6.18.54.1-microsoft-standard-WSL2"));
        assert!(!is_wsl("Linux version 6.8.0-45-generic"));
    }

    #[test]
    fn hint_names_distro_and_avoids_sparse() {
        let h = hint("Ubuntu");
        assert!(h.contains("wsl --export Ubuntu Ubuntu-backup.tar"));
        assert!(h.contains("DistributionName -eq 'Ubuntu'"));
        assert!(!h.contains("set-sparse"));
    }
}
