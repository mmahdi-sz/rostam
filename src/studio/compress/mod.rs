//! Video Compression module for Photo & Video Magic Studio (`studio_compress`).
//!
//! Provides interactive UI for tuning codec (h264, h265, vp9, av1), resolution,
//! framerate (FPS), and bitrate ratio, with Redis session state storage and CPU Broker execution.

pub mod calc;
pub mod handle;
pub mod runner;
pub mod session;
pub mod ui;

#[allow(unused_imports)]
pub use calc::{
    calculate_estimated_size_mb, calculate_target_bitrate_kbps, calculate_target_dimensions,
    compute_vmaf_score, format_eta_hms,
};
#[allow(unused_imports)]
pub use handle::{enter_compress_prompt, handle_compress_cb, handle_video_upload};
#[allow(unused_imports)]
pub use runner::start_compression_job;
#[allow(unused_imports)]
pub use session::{
    CompressSession, SESSION_TTL_SECS, clear_session, load_session, redis_conn, redis_key,
    save_session,
};
#[allow(unused_imports)]
pub use ui::{build_compress_keyboard, build_compress_text, send_compress_prompt_new_msg};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_target_dimensions() {
        // Landscape 1920x1080
        assert_eq!(calculate_target_dimensions(1920, 1080, 1080), (1920, 1080));
        assert_eq!(calculate_target_dimensions(1920, 1080, 720), (1280, 720));
        assert_eq!(calculate_target_dimensions(1920, 1080, 480), (854, 480));

        // Portrait 1080x1920 (e.g. Reels, Shorts)
        assert_eq!(calculate_target_dimensions(1080, 1920, 1080), (1080, 1920));
        assert_eq!(calculate_target_dimensions(1080, 1920, 720), (720, 1280));
        assert_eq!(calculate_target_dimensions(1080, 1920, 480), (480, 854));

        // Square 1080x1080
        assert_eq!(calculate_target_dimensions(1080, 1080, 1080), (1080, 1080));
        assert_eq!(calculate_target_dimensions(1080, 1080, 720), (720, 720));

        // Custom 4:3 1440x1080
        assert_eq!(calculate_target_dimensions(1440, 1080, 720), (960, 720));
    }

    #[test]
    fn test_calculate_target_bitrate_and_estimated_size() {
        let session = CompressSession {
            file_id: "fid".into(),
            filename: "v.mp4".into(),
            orig_w: 1920,
            orig_h: 1080,
            orig_fps: 30,
            orig_bitrate: 2_000_000,
            orig_codec: "h264".into(),
            orig_size_bytes: 30_000_000,
            duration_secs: 120,
            codec: "h264".into(),
            res_h: 720,
            fps: 30,
            br_ratio: 75,
        };

        let target_kbps = calculate_target_bitrate_kbps(&session, 720, 75);
        assert!(target_kbps > 0);
        let est_mb = calculate_estimated_size_mb(&session, 720, 75);
        assert!(est_mb > 0.0);

        // Test portrait video bitrate calculation
        let portrait_session = CompressSession {
            file_id: "fid_portrait".into(),
            filename: "portrait.mp4".into(),
            orig_w: 1080,
            orig_h: 1920,
            orig_fps: 30,
            orig_bitrate: 2_000_000,
            orig_codec: "h264".into(),
            orig_size_bytes: 30_000_000,
            duration_secs: 120,
            codec: "h264".into(),
            res_h: 1080,
            fps: 30,
            br_ratio: 100,
        };
        let p_kbps_1080 = calculate_target_bitrate_kbps(&portrait_session, 1080, 100);
        assert_eq!(p_kbps_1080, 2000);
        let p_kbps_720 = calculate_target_bitrate_kbps(&portrait_session, 720, 100);
        assert!(p_kbps_720 < p_kbps_1080);
    }

    #[test]
    fn test_format_eta_hms() {
        assert_eq!(format_eta_hms(45), "45 ثانیه");
        assert_eq!(format_eta_hms(970), "16 دقیقه و 10 ثانیه");
        assert_eq!(format_eta_hms(3912), "1 ساعت و 5 دقیقه و 12 ثانیه");
    }

    #[test]
    fn test_status_downloading_no_raw_braces() {
        // Reproduces the MarkdownV2 parse error: "Character '{' is reserved"
        // that fires in handle_video_upload when the first send_message call fails.
        let status_raw = crate::i18n::tf(
            "studio.compress.status_downloading",
            &[("elapsed", &crate::i18n::md_escape("0s")), ("detail", "")],
        );
        let status_text = crate::i18n::apply_premium_to_md(&status_raw);
        println!("status_text = {:?}", status_text);
        assert!(
            !status_text.contains('{') && !status_text.contains('}'),
            "MarkdownV2 status text still has raw braces: {:?}",
            status_text
        );

        // Also test via start_compression_job path: t() without tf() leaves placeholders
        let bad_text =
            crate::i18n::apply_premium_to_md(&crate::i18n::t("studio.compress.status_downloading"));
        println!("bad_text (t without tf) = {:?}", bad_text);
        let has_braces = bad_text.contains('{') || bad_text.contains('}');
        println!(
            "Has unescaped braces (start_compression_job bug): {}",
            has_braces
        );
    }
}
