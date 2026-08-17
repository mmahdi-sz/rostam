//! Post-processing module for Pacman (.pkg.tar.zst) packages produced by fpm.
//!
//! Fixes Pacman pkgver hyphen violation by splitting version and revision into `pkgver` and `pkgrel`.
//! Also cleans redundant inner shebangs and strips unnecessary `sudo` calls from .INSTALL scriptlets.
//! Also maps Debian/RPM cross-distro package dependencies to Arch Linux pacman names and trims trailing whitespace.

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Maps cross-distro package dependency names (Debian/RPM/Ubuntu) to official Arch Linux pacman package names.
pub fn map_package_name(name: &str) -> &str {
    let lower = name.to_lowercase();
    match lower.as_str() {
        // GTK & Desktop
        "libgtk-3-0" | "libgtk-3-0g" | "gtk3-devel" => "gtk3",
        "libgtk2.0-0" | "gtk2-devel" => "gtk2",
        "libgtk-4-1" | "gtk4-devel" => "gtk4",
        "libnotify4" | "libnotify-devel" => "libnotify",
        "libglib2.0-0" | "glib2-devel" => "glib2",

        // Security & Crypto
        "libnss3" | "nss-devel" => "nss",
        "libnspr4" | "nspr-devel" => "nspr",
        "libssl1.1" | "libssl3" | "openssl-devel" => "openssl",
        "libsecret-1-0" => "libsecret",
        "libgnome-keyring0" => "libgnome-keyring",

        // Sound
        "libasound2" | "alsa-lib-devel" => "alsa-lib",
        "libpulse0" | "pulseaudio-libs" => "libpulse",
        "libcanberra-gtk3-0" | "canberra-gtk-module" => "libcanberra",

        // Printing & Utilities
        "libcups2" | "cups-libs" => "cups",
        "libcurl4" | "libcurl3" => "curl",
        "zlib1g" => "zlib",
        "libexpat1" => "expat",
        "libdbus-1-3" => "dbus",

        // X11, Wayland & Graphics
        "libxtst6" => "libxtst",
        "libatspi2.0-0" | "libatspi0" => "at-spi2-core",
        "libdrm2" => "libdrm",
        "libgbm1" | "mesa-libgbm" => "mesa",
        "libgl1-mesa-glx" | "libgl1" | "libgl" => "mesa",
        "libxcb-dri3-0" | "libxcb-render0" | "libxcb-shape0" | "libxcb-xfixes0" | "libxcb-shm0"
        | "libxcb1" => "libxcb",
        "libxcb-keysyms1" => "xcb-util-keysyms",
        "libxcb-image0" => "xcb-util-image",
        "libxcb-wm1" => "xcb-util-wm",
        "libx11-6" | "libx11-xcb1" => "libx11",
        "libxcomposite1" => "libxcomposite",
        "libxdamage1" => "libxdamage",
        "libxext6" => "libxext",
        "libxfixes3" => "libxfixes",
        "librandr2" | "libxrandr2" => "libxrandr",
        "libxrender1" => "libxrender",
        "libxss1" => "libxss",
        "libxi6" => "libxi",
        "libxkbcommon0" | "libxkbcommon-x11-0" => "libxkbcommon",
        "libxcursor1" => "libxcursor",
        "libxinerama1" => "libxinerama",
        "libxkbfile1" => "libxkbfile",
        "libappindicator3-1" | "libappindicator1" => "libappindicator-gtk3",

        _ => name,
    }
}

/// Sanitizes a `depend = ...` value from .PKGINFO:
/// - Trims leading and trailing whitespace.
/// - Maps Debian/RPM package names to Arch Linux pacman names.
/// - Preserves version requirements (e.g. `>= 3.0`).
pub fn map_or_sanitize_dep(val: &str) -> Option<String> {
    let trimmed = val.trim();
    if trimmed.is_empty() {
        return None;
    }

    let (name, ver_spec): (&str, Option<String>) =
        if let Some(idx) = trimmed.find(|c: char| c == '>' || c == '<' || c == '=') {
            let (n, v) = trimmed.split_at(idx);
            (n.trim(), Some(v.trim().to_string()))
        } else {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.is_empty() {
                return None;
            }
            (
                parts[0],
                if parts.len() > 1 {
                    Some(parts[1..].join(" "))
                } else {
                    None
                },
            )
        };

    let arch_name = map_package_name(name);

    match ver_spec {
        Some(ver) if !ver.is_empty() => Some(format!("{arch_name} {ver}")),
        _ => Some(arch_name.to_string()),
    }
}

pub fn fix_pacman_pkginfo(path: &Path) -> std::io::Result<()> {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(e) => return Err(e),
    };

    let zdec = match zstd::Decoder::new(file) {
        Ok(d) => d,
        Err(e) => return Err(e),
    };

    let mut archive = tar::Archive::new(zdec);

    let temp_path = path.with_extension("tmp_fix.pkg.tar.zst");
    let out_file = File::create(&temp_path)?;
    let zenc = zstd::Encoder::new(out_file, 3)?.auto_finish();
    let mut builder = tar::Builder::new(zenc);

    let mut modified_any = false;

    let entries = match archive.entries() {
        Ok(e) => e,
        Err(e) => {
            let _ = std::fs::remove_file(&temp_path);
            return Err(e);
        }
    };

    for entry_res in entries {
        let mut entry = entry_res?;
        let path_in_tar = entry.path()?.to_path_buf();
        let header = entry.header().clone();

        if path_in_tar == Path::new(".PKGINFO") {
            let mut content = String::new();
            if entry.read_to_string(&mut content).is_ok() {
                let mut new_lines = Vec::new();
                let mut seen_depends = HashSet::new();

                for line in content.lines() {
                    let trimmed = line.trim();
                    // Strip invalid 'pkgrel =' standalone lines in .PKGINFO
                    if trimmed.starts_with("pkgrel =") {
                        modified_any = true;
                        continue;
                    }

                    if trimmed.starts_with("pkgver =") {
                        let parts: Vec<&str> = line.splitn(2, '=').collect();
                        if parts.len() == 2 {
                            let val = parts[1].trim();
                            let count_hyphens = val.chars().filter(|&c| c == '-').count();

                            let sanitized_ver = match count_hyphens {
                                0 => format!("{val}-1"),
                                1 => val.to_string(),
                                _ => {
                                    if let Some((ver, rel)) = val.rsplit_once('-') {
                                        let ver_clean = ver.replace('-', "+");
                                        format!("{ver_clean}-{rel}")
                                    } else {
                                        val.to_string()
                                    }
                                }
                            };

                            if sanitized_ver != val {
                                modified_any = true;
                            }
                            new_lines.push(format!("pkgver = {sanitized_ver}"));
                            continue;
                        }
                    }

                    if trimmed.starts_with("depend =") {
                        modified_any = true;
                        let parts: Vec<&str> = line.splitn(2, '=').collect();
                        if parts.len() == 2 {
                            let val = parts[1];
                            if let Some(clean_dep) = map_or_sanitize_dep(val) {
                                if seen_depends.insert(clean_dep.clone()) {
                                    new_lines.push(format!("depend = {clean_dep}"));
                                }
                            }
                        }
                        continue;
                    }

                    new_lines.push(line.to_string());
                }

                let new_bytes = new_lines.join("\n").into_bytes();
                let mut new_header = header.clone();
                new_header.set_size(new_bytes.len() as u64);
                new_header.set_cksum();
                builder.append(&new_header, &new_bytes[..])?;
            } else {
                builder.append_data(&mut header.clone(), path_in_tar, entry)?;
            }
        } else if path_in_tar == Path::new(".INSTALL") {
            let mut content = String::new();
            if entry.read_to_string(&mut content).is_ok() {
                let mut new_lines = Vec::new();
                for (idx, line) in content.lines().enumerate() {
                    let trimmed = line.trim();
                    if idx > 0
                        && (trimmed == "#!/usr/bin/env sh"
                            || trimmed == "#!/bin/sh"
                            || trimmed == "#!/bin/bash")
                    {
                        continue;
                    }
                    let cleaned = line.replace("sudo ", "");
                    new_lines.push(cleaned);
                }

                let new_bytes = new_lines.join("\n").into_bytes();
                let mut new_header = header.clone();
                new_header.set_size(new_bytes.len() as u64);
                new_header.set_cksum();
                builder.append(&new_header, &new_bytes[..])?;
                modified_any = true;
            } else {
                builder.append_data(&mut header.clone(), path_in_tar, entry)?;
            }
        } else {
            builder.append_data(&mut header.clone(), path_in_tar, entry)?;
        }
    }

    builder.finish()?;

    if modified_any {
        std::fs::rename(&temp_path, path)?;
    } else {
        let _ = std::fs::remove_file(&temp_path);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pkgver_sanitization() {
        fn sanitize(val: &str) -> String {
            let count_hyphens = val.chars().filter(|&c| c == '-').count();
            match count_hyphens {
                0 => format!("{val}-1"),
                1 => val.to_string(),
                _ => {
                    if let Some((ver, rel)) = val.rsplit_once('-') {
                        let ver_clean = ver.replace('-', "+");
                        format!("{ver_clean}-{rel}")
                    } else {
                        val.to_string()
                    }
                }
            }
        }

        assert_eq!(sanitize("26.6.1-1"), "26.6.1-1");
        assert_eq!(sanitize("26.6.1"), "26.6.1-1");
        assert_eq!(sanitize("4.1.1-40101-1"), "4.1.1+40101-1");
    }

    #[test]
    fn test_dep_mapping_and_sanitization() {
        assert_eq!(
            map_or_sanitize_dep("libgtk-3-0  "),
            Some("gtk3".to_string())
        );
        assert_eq!(
            map_or_sanitize_dep("libnotify4  "),
            Some("libnotify".to_string())
        );
        assert_eq!(map_or_sanitize_dep("libnss3  "), Some("nss".to_string()));
        assert_eq!(
            map_or_sanitize_dep("libxtst6  "),
            Some("libxtst".to_string())
        );
        assert_eq!(
            map_or_sanitize_dep("xdg-utils  "),
            Some("xdg-utils".to_string())
        );
        assert_eq!(
            map_or_sanitize_dep("libatspi2.0-0  "),
            Some("at-spi2-core".to_string())
        );
        assert_eq!(map_or_sanitize_dep("libdrm2  "), Some("libdrm".to_string()));
        assert_eq!(map_or_sanitize_dep("libgbm1  "), Some("mesa".to_string()));
        assert_eq!(
            map_or_sanitize_dep("libxcb-dri3-0  "),
            Some("libxcb".to_string())
        );
        assert_eq!(
            map_or_sanitize_dep("libgtk-3-0 >= 3.24.0  "),
            Some("gtk3 >= 3.24.0".to_string())
        );
    }
}
