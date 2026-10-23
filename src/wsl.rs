/// `proc_version` is the contents of `/proc/version`.
pub fn is_wsl(proc_version: &str) -> bool {
    proc_version.to_lowercase().contains("microsoft")
}

/// Steps to return freed space to Windows. Printed only; never run.
/// Deliberately not `--set-sparse`: WSL disabled it over data-corruption reports.
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
