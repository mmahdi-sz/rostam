//! PDF compression subsystem using Ghostscript presets.

mod handle;
pub use handle::{
    CB_PDF_CANCEL, CB_PDF_LEVEL_PREFIX, CB_PDF_MODE_ADVANCED, CB_PDF_MODE_SIMPLE,
    CB_TOOLS_PDF_COMPRESS, enter_pdf_compress, handle_pdf_cancel, handle_pdf_file,
    handle_pdf_level, handle_pdf_mode_simple,
};
