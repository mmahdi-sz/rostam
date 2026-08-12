-- Add feature, speed tracking, and file count to stats_downloads
ALTER TABLE stats_downloads ADD COLUMN IF NOT EXISTS feature TEXT DEFAULT 'youtube';
ALTER TABLE stats_downloads ADD COLUMN IF NOT EXISTS download_speed_bps BIGINT;
ALTER TABLE stats_downloads ADD COLUMN IF NOT EXISTS upload_speed_bps BIGINT;
ALTER TABLE stats_downloads ADD COLUMN IF NOT EXISTS file_count INTEGER DEFAULT 1;
