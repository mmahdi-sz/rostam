import json

with open("config/i18n.json", "r", encoding="utf-8") as f:
    data = json.load(f)

# Update main_text and more_text for FA
data["fa"]["panel"]["main_text"] = "👤 <b>پنل کاربری</b>\nمقام: <b>{rank}</b>  ·  {expiry}\n\n<b>📶 ترافیک روزانه</b>\n{bar_d}  {used_d} مصرف  |  {left_d} باقی\n\n<b>📅 ترافیک ماهانه</b>\n{bar_m}  {used_m} مصرف  |  {left_m} باقی"
data["fa"]["panel"]["more_text"] = "📊 <b>سهمیه‌های دیگر اکانت</b>\n\n<b>🎙️ رونویسی سریع</b>\nروزانه: {b_sf_d}  {u_sf_d} / {l_sf_d}\nهفتگی: {b_sf_w}  {u_sf_w} / {l_sf_w}\n\n<b>✨ رونویسی دقیق</b>\nروزانه: {b_sa_d}  {u_sa_d} / {l_sa_d}\nهفتگی: {b_sa_w}  {u_sa_w} / {l_sa_w}\n\n<b>🎚️ حذف نویز</b>\nروزانه: {b_dn_d}  {u_dn_d} / {l_dn_d}\nهفتگی: {b_dn_w}  {u_dn_w} / {l_dn_w}\n\n<b>🎵 جداسازی</b>\nروزانه: {b_sep_d}  {u_sep_d} / {l_sep_d}\nهفتگی: {b_sep_w}  {u_sep_w} / {l_sep_w}\n\n<b>🖼️ افزایش کیفیت (هفتگی)</b>\n×۲: {b_u2}  {v_u2}/{l_u2}  |  ×۳: {b_u3}  {v_u3}/{l_u3}  |  ×۴: {b_u4}  {v_u4}/{l_u4}\n\n<b>✂️ حذف پس‌زمینه (هفتگی)</b>\n{b_nobg}  {u_nobg}/{l_nobg}\n\n<b>🎨 رنگی‌کردن عکس (هفتگی)</b>\n{b_deold}  {u_deold}/{l_deold}\n\n<b>🗣️ تبدیل متن به صدا (هفتگی)</b>\n{b_tts}  {u_tts} / {l_tts}"

# EN
data["en"]["panel"]["main_text"] = "👤 <b>User Panel</b>\nRank: <b>{rank}</b>  ·  {expiry}\n\n<b>📶 Daily traffic</b>\n{bar_d}  {used_d} used  |  {left_d} left\n\n<b>📅 Monthly traffic</b>\n{bar_m}  {used_m} used  |  {left_m} left"
data["en"]["panel"]["more_text"] = "📊 <b>Other account quotas</b>\n\n<b>🎙️ Fast transcription</b>\nDaily: {b_sf_d}  {u_sf_d} / {l_sf_d}\nWeekly: {b_sf_w}  {u_sf_w} / {l_sf_w}\n\n<b>✨ Accurate transcription</b>\nDaily: {b_sa_d}  {u_sa_d} / {l_sa_d}\nWeekly: {b_sa_w}  {u_sa_w} / {l_sa_w}\n\n<b>🎚️ Noise removal</b>\nDaily: {b_dn_d}  {u_dn_d} / {l_dn_d}\nWeekly: {b_dn_w}  {u_dn_w} / {l_dn_w}\n\n<b>🎵 Separation</b>\nDaily: {b_sep_d}  {u_sep_d} / {l_sep_d}\nWeekly: {b_sep_w}  {u_sep_w} / {l_sep_w}\n\n<b>🖼️ Upscale (weekly)</b>\n×2: {b_u2}  {v_u2}/{l_u2}  |  ×3: {b_u3}  {v_u3}/{l_u3}  |  ×4: {b_u4}  {v_u4}/{l_u4}\n\n<b>✂️ Remove Background (weekly)</b>\n{b_nobg}  {u_nobg}/{l_nobg}\n\n<b>🎨 Colorize Photo (weekly)</b>\n{b_deold}  {u_deold}/{l_deold}\n\n<b>🗣️ Text to Speech (weekly)</b>\n{b_tts}  {u_tts} / {l_tts}"

# IT
data["it"]["panel"]["main_text"] = "👤 <b>Pannello utente</b>\nRango: <b>{rank}</b>  ·  {expiry}\n\n<b>📶 Traffico giornaliero</b>\n{bar_d}  {used_d} usato  |  {left_d} rimasto\n\n<b>📅 Traffico mensile</b>\n{bar_m}  {used_m} usato  |  {left_m} rimasto"
data["it"]["panel"]["more_text"] = "📊 <b>Altre quote dell'account</b>\n\n<b>🎙️ Trascrizione rapida</b>\nGiornaliera: {b_sf_d}  {u_sf_d} / {l_sf_d}\nSettimanale: {b_sf_w}  {u_sf_w} / {l_sf_w}\n\n<b>✨ Trascrizione accurata</b>\nGiornaliera: {b_sa_d}  {u_sa_d} / {l_sa_d}\nSettimanale: {b_sa_w}  {u_sa_w} / {l_sa_w}\n\n<b>🎚️ Rimozione rumore</b>\nGiornaliera: {b_dn_d}  {u_dn_d} / {l_dn_d}\nSettimanale: {b_dn_w}  {u_dn_w} / {l_dn_w}\n\n<b>🎵 Separazione</b>\nGiornaliera: {b_sep_d}  {u_sep_d} / {l_sep_d}\nSettimanale: {b_sep_w}  {u_sep_w} / {l_sep_w}\n\n<b>🖼️ Upscale (settimanale)</b>\n×2: {b_u2}  {v_u2}/{l_u2}  |  ×3: {b_u3}  {v_u3}/{l_u3}  |  ×4: {b_u4}  {v_u4}/{l_u4}\n\n<b>✂️ Rimuovi Sfondo (settimanale)</b>\n{b_nobg}  {u_nobg}/{l_nobg}\n\n<b>🎨 Colora Foto (settimanale)</b>\n{b_deold}  {u_deold}/{l_deold}\n\n<b>🗣️ Sintesi Vocale (settimanale)</b>\n{b_tts}  {u_tts} / {l_tts}"

# RU
data["ru"]["panel"]["main_text"] = "👤 <b>Панель пользователя</b>\nРанг: <b>{rank}</b>  ·  {expiry}\n\n<b>📶 Дневной трафик</b>\n{bar_d}  {used_d} израсходовано  |  {left_d} осталось\n\n<b>📅 Месячный трафик</b>\n{bar_m}  {used_m} израсходовано  |  {left_m} осталось"
data["ru"]["panel"]["more_text"] = "📊 <b>Другие квоты</b>\n\n<b>🎙️ Быстрая транскрипция</b>\nДень: {b_sf_d}  {u_sf_d} / {l_sf_d}\nНеделя: {b_sf_w}  {u_sf_w} / {l_sf_w}\n\n<b>✨ Точная транскрипция</b>\nДень: {b_sa_d}  {u_sa_d} / {l_sa_d}\nНеделя: {b_sa_w}  {u_sa_w} / {l_sa_w}\n\n<b>🎚️ Удаление шума</b>\nДень: {b_dn_d}  {u_dn_d} / {l_dn_d}\nНеделя: {b_dn_w}  {u_dn_w} / {l_dn_w}\n\n<b>🎵 Разделение</b>\nДень: {b_sep_d}  {u_sep_d} / {l_sep_d}\nНеделя: {b_sep_w}  {u_sep_w} / {l_sep_w}\n\n<b>🖼️ Масштабирование (неделя)</b>\n×2: {b_u2}  {v_u2}/{l_u2}  |  ×3: {b_u3}  {v_u3}/{l_u3}  |  ×4: {b_u4}  {v_u4}/{l_u4}\n\n<b>✂️ Удалить фон (неделя)</b>\n{b_nobg}  {u_nobg}/{l_nobg}\n\n<b>🎨 Раскрасить фото (неделя)</b>\n{b_deold}  {u_deold}/{l_deold}\n\n<b>🗣️ Текст в речь (неделя)</b>\n{b_tts}  {u_tts} / {l_tts}"

# Features updates
for lang, features in [
    ("fa", {
        "dalavar": "✂️ حذف پس‌زمینه (هفتگی): <b>۳</b> | 🎨 رنگی‌کردن عکس: <b>۳</b>\n🗣️ تبدیل متن به صدا: <b>۳۰ دقیقه هفتگی</b>",
        "sepahbod": "✂️ حذف پس‌زمینه (هفتگی): <b>۳</b> | 🎨 رنگی‌کردن عکس: <b>۳</b>\n🗣️ تبدیل متن به صدا: <b>۳۰ دقیقه هفتگی</b>",
        "esfandyar": "✂️ حذف پس‌زمینه (هفتگی): <b>۳</b> | 🎨 رنگی‌کردن عکس: <b>۳</b>\n🗣️ تبدیل متن به صدا: <b>۳۰ دقیقه هفتگی</b>",
        "sohrab": "✂️ حذف پس‌زمینه (هفتگی): <b>۳۰</b> | 🎨 رنگی‌کردن عکس: <b>۱۵</b>\n🗣️ تبدیل متن به صدا: <b>۱۰۰ دقیقه هفتگی</b>",
        "rostam": "✂️ حذف پس‌زمینه (هفتگی): <b>۱۵۰</b> | 🎨 رنگی‌کردن عکس: <b>۱۰۰</b>\n🗣️ تبدیل متن به صدا: <b>۶۰۰ دقیقه هفتگی</b>"
    }),
    ("en", {
        "dalavar": "✂️ Remove BG (weekly): <b>3</b> | 🎨 Colorize: <b>3</b>\n🗣️ Text to Speech: <b>30 mins weekly</b>",
        "sepahbod": "✂️ Remove BG (weekly): <b>3</b> | 🎨 Colorize: <b>3</b>\n🗣️ Text to Speech: <b>30 mins weekly</b>",
        "esfandyar": "✂️ Remove BG (weekly): <b>3</b> | 🎨 Colorize: <b>3</b>\n🗣️ Text to Speech: <b>30 mins weekly</b>",
        "sohrab": "✂️ Remove BG (weekly): <b>30</b> | 🎨 Colorize: <b>15</b>\n🗣️ Text to Speech: <b>100 mins weekly</b>",
        "rostam": "✂️ Remove BG (weekly): <b>150</b> | 🎨 Colorize: <b>100</b>\n🗣️ Text to Speech: <b>600 mins weekly</b>"
    }),
    ("it", {
        "dalavar": "✂️ Rimuovi Sfondo (sett.): <b>3</b> | 🎨 Colora: <b>3</b>\n🗣️ Sintesi Vocale: <b>30 min settimanale</b>",
        "sepahbod": "✂️ Rimuovi Sfondo (sett.): <b>3</b> | 🎨 Colora: <b>3</b>\n🗣️ Sintesi Vocale: <b>30 min settimanale</b>",
        "esfandyar": "✂️ Rimuovi Sfondo (sett.): <b>3</b> | 🎨 Colora: <b>3</b>\n🗣️ Sintesi Vocale: <b>30 min settimanale</b>",
        "sohrab": "✂️ Rimuovi Sfondo (sett.): <b>30</b> | 🎨 Colora: <b>15</b>\n🗣️ Sintesi Vocale: <b>100 min settimanale</b>",
        "rostam": "✂️ Rimuovi Sfondo (sett.): <b>150</b> | 🎨 Colora: <b>100</b>\n🗣️ Sintesi Vocale: <b>600 min settimanale</b>"
    }),
    ("ru", {
        "dalavar": "✂️ Удалить фон (неделя): <b>3</b> | 🎨 Раскрасить: <b>3</b>\n🗣️ Текст в речь: <b>30 мин/неделя</b>",
        "sepahbod": "✂️ Удалить фон (неделя): <b>3</b> | 🎨 Раскрасить: <b>3</b>\n🗣️ Текст в речь: <b>30 мин/неделя</b>",
        "esfandyar": "✂️ Удалить фон (неделя): <b>3</b> | 🎨 Раскрасить: <b>3</b>\n🗣️ Текст в речь: <b>30 мин/неделя</b>",
        "sohrab": "✂️ Удалить фон (неделя): <b>30</b> | 🎨 Раскрасить: <b>15</b>\n🗣️ Текст в речь: <b>100 мин/неделя</b>",
        "rostam": "✂️ Удалить фон (неделя): <b>150</b> | 🎨 Раскрасить: <b>100</b>\n🗣️ Текст в речь: <b>600 мин/неделя</b>"
    }),
]:
    for r in ["dalavar", "sepahbod", "esfandyar", "sohrab", "rostam"]:
        original = data[lang]["rank"]["features"][r]
        # Remove AI models mention if present
        if "✨" in original and ("جمنای" in original or "Gemini" in original):
            parts = original.split("\n")
            parts = [p for p in parts if not ("✨" in p and ("جمنای" in p or "Gemini" in p))]
            original = "\n".join(parts)
        
        # Append the new AI features
        data[lang]["rank"]["features"][r] = original + "\n" + features[r]

with open("config/i18n.json", "w", encoding="utf-8") as f:
    json.dump(data, f, ensure_ascii=False, indent=2)
