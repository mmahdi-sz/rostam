# systemd units

Canonical files live in `dev/ops/systemd/system/` and `dev/ops/systemd/user/`.
The live files in `/etc/systemd/system/` and `/home/mahdi/.config/systemd/user/`
are symlinks to these sources, including the production memory drop-in.

After editing system units, run `sudo systemctl daemon-reload`.
For the dev user unit, run `systemctl --user daemon-reload`.
Restart the affected service to apply changes to its running process.
Editing a symlink target alone does not reload systemd or restart a process.

Telegram API credentials stay in root-owned mode-0600 files under
`/etc/ros-telegram-bot/`, referenced with `EnvironmentFile=`. Bot and watchdog
`.env` files remain at their existing external paths. Never commit credentials.

Validate with `sudo systemd-analyze verify ops/systemd/system/*.service ops/systemd/system/*.timer`
and `systemd-analyze --user verify ops/systemd/user/rostam_dev.service`.
