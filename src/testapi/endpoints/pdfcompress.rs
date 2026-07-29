use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct PdfCompressReq {
    pub filename: String,
    pub level: String,
}

#[derive(Serialize)]
pub struct PdfCompressResp {
    pub ok: bool,
    pub filename: String,
    pub applied_level: String,
    pub gs_flag: String,
}

pub async fn test_pdf_compress(
    Json(req): Json<PdfCompressReq>,
) -> (axum::http::StatusCode, Json<PdfCompressResp>) {
    let gs_flag = match req.level.as_str() {
        "screen" => "-dPDFSETTINGS=/screen",
        "ebook" => "-dPDFSETTINGS=/ebook",
        "printer" => "-dPDFSETTINGS=/printer",
        "prepress" => "-dPDFSETTINGS=/prepress",
        _ => "-dPDFSETTINGS=/ebook",
    };

    (
        axum::http::StatusCode::OK,
        Json(PdfCompressResp {
            ok: req.filename.ends_with(".pdf"),
            filename: req.filename,
            applied_level: req.level,
            gs_flag: gs_flag.to_string(),
        }),
    )
}
