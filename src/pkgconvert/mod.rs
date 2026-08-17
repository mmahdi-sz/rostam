//! Universal Package Format Converter (`pkgconvert`) subsystem.
//!
//! Supports conversion between .deb, .rpm, and pacman (.pkg.tar.zst) packages
//! using Alien and fpm isolated within a bubblewrap (bwrap) sandbox.

pub mod detect;
pub mod engine;
pub mod handle;
pub mod pacman_fix;
pub mod validate;

#[allow(unused_imports)]
pub use detect::PkgFormat;
#[allow(unused_imports)]
pub use engine::TargetFmt;
pub use handle::{enter_pkgconvert, handle_pkg_callback, handle_pkg_file, handle_pkg_jobcancel};
