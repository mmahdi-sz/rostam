//! TestAPI endpoints for the Package Converter feature (`pkg`).

use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use axum::Json;
use serde::{Deserialize, Serialize};

use crate::i18n::t;
use crate::pkgconvert::detect::PkgFormat;
use crate::pkgconvert::engine::{TargetFmt, select_tool};
use crate::pkgconvert::validate::{ValidateError, validate_package};
use crate::rank::types::Rank;

#[derive(Deserialize)]
pub struct ValidateReq {
    pub format: String,
    pub test_case: String,
}

#[derive(Serialize)]
pub struct ValidateResp {
    pub ok: bool,
    pub error: Option<String>,
    pub error_kind: Option<String>,
}

pub async fn test_pkg_validate(
    Json(req): Json<ValidateReq>,
) -> (axum::http::StatusCode, Json<ValidateResp>) {
    let fmt = PkgFormat::from_str(&req.format).unwrap_or(PkgFormat::Deb);
    let rand_id: u64 = rand::random();
    let temp_dir_path = std::env::temp_dir().join(format!("test_pkg_val_{rand_id}"));

    if let Err(e) = std::fs::create_dir_all(&temp_dir_path) {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(ValidateResp {
                ok: false,
                error: Some(format!("mkdir tempdir failed: {e}")),
                error_kind: Some("SystemError".to_string()),
            }),
        );
    }

    let sample_path = temp_dir_path.join(format!("sample{}", fmt.display_ext()));

    if let Err(e) = create_test_package(&sample_path, fmt, &req.test_case) {
        std::fs::remove_dir_all(&temp_dir_path).ok();
        return (
            axum::http::StatusCode::BAD_REQUEST,
            Json(ValidateResp {
                ok: false,
                error: Some(format!("Failed to build test package: {e}")),
                error_kind: Some("BuildFailed".to_string()),
            }),
        );
    }

    let val_res = validate_package(&sample_path, fmt, 0).await;
    std::fs::remove_dir_all(&temp_dir_path).ok();

    match val_res {
        Ok(()) => (
            axum::http::StatusCode::OK,
            Json(ValidateResp {
                ok: true,
                error: None,
                error_kind: None,
            }),
        ),
        Err(err) => {
            let (msg, kind) = match err {
                ValidateError::TooLarge(sz) => (format!("Too large: {sz}"), "TooLarge"),
                ValidateError::TooManyEntries(cnt) => {
                    (format!("Too many entries: {cnt}"), "TooManyEntries")
                }
                ValidateError::PathTraversal(p) => {
                    (format!("Path traversal: {p}"), "PathTraversal")
                }
                ValidateError::SymlinkEscape(e, target) => {
                    (format!("Symlink escape: {e} -> {target}"), "SymlinkEscape")
                }
                ValidateError::FileTooLarge(f) => (format!("File too large: {f}"), "FileTooLarge"),
                ValidateError::Timeout => ("Validation timeout".to_string(), "Timeout"),
                ValidateError::ParseError(p) => (format!("Parse error: {p}"), "ParseError"),
            };
            (
                axum::http::StatusCode::OK,
                Json(ValidateResp {
                    ok: false,
                    error: Some(msg),
                    error_kind: Some(kind.to_string()),
                }),
            )
        }
    }
}

fn create_test_package(path: &Path, fmt: PkgFormat, test_case: &str) -> std::io::Result<()> {
    match fmt {
        PkgFormat::Deb => {
            let mut ar_builder = ar::Builder::new(File::create(path)?);
            let header = ar::Header::new(b"debian-binary".to_vec(), 4);
            ar_builder.append(&header, &b"2.0\n"[..])?;

            let tar_path = path.with_extension("data.tar");
            {
                let mut tar_builder = tar::Builder::new(File::create(&tar_path)?);
                let content = b"echo hello world";
                let mut h = tar::Header::new_gnu();
                let entry_name = match test_case {
                    "path_traversal" => "../../../etc/passwd",
                    _ => "usr/bin/hello",
                };
                let name_bytes = entry_name.as_bytes();
                h.as_mut_bytes()[..name_bytes.len()].copy_from_slice(name_bytes);
                h.set_size(content.len() as u64);
                h.set_cksum();
                tar_builder.append(&h, &content[..])?;
                tar_builder.finish()?;
            }

            let tar_data = std::fs::read(&tar_path)?;
            let _ = std::fs::remove_file(tar_path);

            let data_header = ar::Header::new(b"data.tar".to_vec(), tar_data.len() as u64);
            ar_builder.append(&data_header, &tar_data[..])?;
        }
        PkgFormat::Pacman => {
            let file = File::create(path)?;
            let zstd_enc = zstd::Encoder::new(file, 1)?;
            let mut tar_builder = tar::Builder::new(zstd_enc);

            let entry_name = match test_case {
                "path_traversal" => "../../../etc/shadow",
                _ => "usr/share/doc/hello/README",
            };
            let content = b"Pacman test package content";
            let mut h = tar::Header::new_gnu();
            h.set_size(content.len() as u64);
            h.set_cksum();
            tar_builder.append_data(&mut h, entry_name, &content[..])?;

            if test_case == "symlink_escape" {
                let mut sym_h = tar::Header::new_gnu();
                sym_h.set_entry_type(tar::EntryType::Symlink);
                sym_h.set_size(0);
                sym_h.set_cksum();
                tar_builder.append_link(&mut sym_h, "usr/bin/evil_link", "../../../etc/shadow")?;
            }

            let zstd_enc = tar_builder.into_inner()?;
            zstd_enc.finish()?;
        }
        PkgFormat::Rpm => {
            // Write a dummy RPM file header
            let mut f = File::create(path)?;
            f.write_all(&[0xED, 0xAB, 0xEE, 0xDB])?; // RPM magic
            f.write_all(&[0u8; 96])?; // dummy RPM lead
        }
    }
    Ok(())
}

#[derive(Deserialize)]
pub struct ConvertReq {
    pub src_fmt: String,
    pub dst_fmt: String,
    pub rank: Option<String>,
    pub quota_exhausted: Option<bool>,
}

#[derive(Serialize)]
pub struct ConvertResp {
    pub paywall_blocked: bool,
    pub quota_blocked: bool,
    pub tool_selected: String,
    pub paywall_text: Option<String>,
    pub quota_text: Option<String>,
    pub daily_limit: u64,
}

pub async fn test_pkg_convert(
    Json(req): Json<ConvertReq>,
) -> (axum::http::StatusCode, Json<ConvertResp>) {
    let src = PkgFormat::from_str(&req.src_fmt).unwrap_or(PkgFormat::Deb);
    let dst = TargetFmt::from_str(&req.dst_fmt).unwrap_or(TargetFmt::Rpm);
    let rank =
        Rank::from_str(&req.rank.unwrap_or_else(|| "dalavar".to_string())).unwrap_or(Rank::Dalavar);

    let tool = select_tool(src, dst);
    let daily_limit = rank.pkgconvert_daily_count();

    if daily_limit == 0 {
        return (
            axum::http::StatusCode::OK,
            Json(ConvertResp {
                paywall_blocked: true,
                quota_blocked: false,
                tool_selected: match tool {
                    crate::pkgconvert::engine::ConversionTool::Alien => "alien".to_string(),
                    crate::pkgconvert::engine::ConversionTool::Fpm => "fpm".to_string(),
                },
                paywall_text: Some(t("pkg.paywall")),
                quota_text: None,
                daily_limit,
            }),
        );
    }

    if req.quota_exhausted.unwrap_or(false) {
        return (
            axum::http::StatusCode::OK,
            Json(ConvertResp {
                paywall_blocked: false,
                quota_blocked: true,
                tool_selected: match tool {
                    crate::pkgconvert::engine::ConversionTool::Alien => "alien".to_string(),
                    crate::pkgconvert::engine::ConversionTool::Fpm => "fpm".to_string(),
                },
                paywall_text: None,
                quota_text: Some(t("pkg.quota_exceeded")),
                daily_limit,
            }),
        );
    }

    (
        axum::http::StatusCode::OK,
        Json(ConvertResp {
            paywall_blocked: false,
            quota_blocked: false,
            tool_selected: match tool {
                crate::pkgconvert::engine::ConversionTool::Alien => "alien".to_string(),
                crate::pkgconvert::engine::ConversionTool::Fpm => "fpm".to_string(),
            },
            paywall_text: None,
            quota_text: None,
            daily_limit,
        }),
    )
}

#[derive(Deserialize)]
pub struct UxReq {
    pub src_fmt: Option<String>,
    pub stage: Option<String>,
}

#[derive(Serialize)]
pub struct ButtonDto {
    pub text: String,
    pub cb_data: String,
}

#[derive(Serialize)]
pub struct UxResp {
    pub prompt_text: String,
    pub detected_text: String,
    pub target_buttons: Vec<ButtonDto>,
    pub stage_text: String,
    pub cancel_button_text: String,
    pub error_texts: HashMap<String, String>,
    pub detected_text_len: usize,
}

pub async fn test_pkg_ux(Json(req): Json<UxReq>) -> (axum::http::StatusCode, Json<UxResp>) {
    let src = PkgFormat::from_str(&req.src_fmt.unwrap_or_else(|| "deb".to_string()))
        .unwrap_or(PkgFormat::Deb);
    let stage = req.stage.unwrap_or_else(|| "converting".to_string());

    let prompt_text = t("pkg.prompt");
    let detected_text = crate::i18n::tf("pkg.detected", &[("fmt", src.display_ext())]);

    let target_buttons = match src {
        PkgFormat::Deb => vec![
            ButtonDto {
                text: t("pkg.convert_btn_rpm"),
                cb_data: "pkg:convert:deb:rpm".to_string(),
            },
            ButtonDto {
                text: t("pkg.convert_btn_pacman"),
                cb_data: "pkg:convert:deb:pacman".to_string(),
            },
        ],
        PkgFormat::Rpm => vec![
            ButtonDto {
                text: t("pkg.convert_btn_deb"),
                cb_data: "pkg:convert:rpm:deb".to_string(),
            },
            ButtonDto {
                text: t("pkg.convert_btn_pacman"),
                cb_data: "pkg:convert:rpm:pacman".to_string(),
            },
        ],
        PkgFormat::Pacman => vec![
            ButtonDto {
                text: t("pkg.convert_btn_deb"),
                cb_data: "pkg:convert:pacman:deb".to_string(),
            },
            ButtonDto {
                text: t("pkg.convert_btn_rpm"),
                cb_data: "pkg:convert:pacman:rpm".to_string(),
            },
        ],
    };

    let stage_text = match stage.as_str() {
        "downloading" => t("pkg.stage.downloading"),
        "validating" => t("pkg.stage.validating"),
        "uploading" => t("pkg.stage.uploading"),
        _ => crate::i18n::tf("pkg.stage.converting", &[("elapsed", "00:15")]),
    };

    let mut error_texts = HashMap::new();
    error_texts.insert(
        "file_too_large".to_string(),
        crate::i18n::tf("pkg.error.file_too_large", &[("max", "200 MB")]),
    );
    error_texts.insert(
        "malicious_archive".to_string(),
        t("pkg.error.malicious_archive"),
    );
    error_texts.insert("convert_failed".to_string(), t("pkg.error.convert_failed"));
    error_texts.insert(
        "unsupported_format".to_string(),
        t("pkg.error.unsupported_format"),
    );

    (
        axum::http::StatusCode::OK,
        Json(UxResp {
            detected_text_len: detected_text.chars().count(),
            prompt_text,
            detected_text,
            target_buttons,
            stage_text,
            cancel_button_text: t("pkg.cancel_btn"),
            error_texts,
        }),
    )
}
