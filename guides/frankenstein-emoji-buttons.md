# frankenstein — Custom Emoji & Colored Buttons Reference

> **frankenstein v0.50.0** | Telegram Bot API **9.4+**  
> نیاز: owner بات باید **Telegram Premium** داشته باشه

---

## 📋 فهرست راهنما

| نیاز | بخش |
|------|-----|
| custom emoji توی متن پیام | [§1 Custom Emoji در متن](#1-custom-emoji-در-متن) |
| custom emoji توی caption عکس/ویدیو | [§2 Custom Emoji در Caption](#2-custom-emoji-در-caption) |
| دکمه رنگی InlineKeyboard | [§3 Colored InlineKeyboardButton](#3-colored-inlinekeyboardbutton) |
| دکمه رنگی ReplyKeyboard | [§4 Colored KeyboardButton](#4-colored-keyboardbutton) |
| custom emoji روی دکمه‌ها | [§5 Custom Emoji روی دکمه](#5-custom-emoji-روی-دکمه) |
| پیدا کردن emoji_id | [§6 پیدا کردن Emoji ID](#6-پیدا-کردن-emoji-id) |
| کپی پیام با custom emoji | [§7 کپی پیام](#7-کپی-پیام-با-custom-emoji) |
| گرفتن همه ایموجی‌های یه پک | [§8 گرفتن کل پک](#8-گرفتن-کل-پک-از-یه-ایموجی) |
| قوانین و محدودیت‌ها | [§9 محدودیت‌ها](#9-محدودیت‌ها) |
| جدول مقایسه همه موارد | [§10 جدول مقایسه](#10-جدول-مقایسه) |

---

## 1. Custom Emoji در متن

برای custom emoji در متن **نباید** از parse_mode استفاده کنی.  
باید از `entities` array استفاده کنی:

```rust
use frankenstein::{
    AsyncTelegramApi, AsyncApi, SendMessageParams,
    MessageEntity, MessageEntityType,
};

let text = "به بات خوش اومدی 👋";
//          offset ↑ اینجا = 18 (UTF-16)

let entities = vec![
    MessageEntity::builder()
        .type_field(MessageEntityType::CustomEmoji)
        .offset(18u16)        // موقعیت کاراکتر توی متن
        .length(1u16)         // همیشه 1
        .custom_emoji_id("EMOJI_ID".to_string())
        .build()
];

let params = SendMessageParams::builder()
    .chat_id(chat_id)
    .text(text)
    .entities(entities)
    // بدون parse_mode!
    .build();

api.send_message(&params).await?;
```

> **نکته offset:** offset بر اساس UTF-16 code units محاسبه میشه.  
> برای فارسی/عربی هر کاراکتر = 1 واحد.  
> برای اکثر ایموجی‌های معمولی = 2 واحد (surrogate pair).

### ترکیب با MarkdownV2
اگه بقیه متن formatting داره، هر دو رو با هم بده:

```rust
let params = SendMessageParams::builder()
    .chat_id(chat_id)
    .text("*سلام* به بات خوش اومدی 👋")
    .parse_mode(ParseMode::MarkdownV2)
    .entities(entities)  // custom emoji جداگانه
    .build();
```

---

## 2. Custom Emoji در Caption

دقیقاً مثل متن — فقط به جای `entities` باید `caption_entities` بدی:

```rust
use frankenstein::{
    SendPhotoParams, InputFile,
    MessageEntity, MessageEntityType,
};

let caption = "👀 عکس جدید آپلود شد";

let caption_entities = vec![
    MessageEntity::builder()
        .type_field(MessageEntityType::CustomEmoji)
        .offset(0u16)
        .length(2u16)  // ایموجی معمولی = 2 واحد UTF-16
        .custom_emoji_id("EMOJI_ID".to_string())
        .build()
];

let params = SendPhotoParams::builder()
    .chat_id(chat_id)
    .photo(InputFile::String("file_id".into()))
    .caption(caption)
    .caption_entities(caption_entities)
    .build();

api.send_photo(&params).await?;
```

> همین pattern برای `SendVideoParams`، `SendDocumentParams`،  
> `SendAudioParams` و بقیه هم کار می‌کنه.

---

## 3. Colored InlineKeyboardButton

از Bot API 9.4، `InlineKeyboardButton` فیلد `style` داره:

```rust
use frankenstein::{
    SendMessageParams,
    InlineKeyboardButton, InlineKeyboardMarkup,
    ReplyMarkup, ButtonStyle,
};

// دکمه سبز
let btn_confirm = InlineKeyboardButton::builder()
    .text("تایید")
    .callback_data("confirm")
    .style(ButtonStyle::Success)
    .build();

// دکمه قرمز
let btn_cancel = InlineKeyboardButton::builder()
    .text("لغو")
    .callback_data("cancel")
    .style(ButtonStyle::Danger)
    .build();

// دکمه آبی
let btn_info = InlineKeyboardButton::builder()
    .text("اطلاعات بیشتر")
    .callback_data("info")
    .style(ButtonStyle::Primary)
    .build();

let keyboard = InlineKeyboardMarkup::builder()
    .inline_keyboard(vec![
        vec![btn_confirm, btn_cancel],
        vec![btn_info],
    ])
    .build();

let params = SendMessageParams::builder()
    .chat_id(chat_id)
    .text("یه گزینه انتخاب کن:")
    .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
    .build();

api.send_message(&params).await?;
```

### ButtonStyle مقادیر:

| Variant | رنگ | کاربرد پیشنهادی |
|---------|-----|-----------------|
| `ButtonStyle::Primary` | آبی | عملیات اصلی |
| `ButtonStyle::Success` | سبز | تایید، موفقیت |
| `ButtonStyle::Danger` | قرمز | حذف، لغو، هشدار |

---

## 4. Colored KeyboardButton

`KeyboardButton` (reply keyboard) هم همین فیلدها رو داره:

```rust
use frankenstein::{
    SendMessageParams,
    KeyboardButton, ReplyKeyboardMarkup,
    ReplyMarkup, ButtonStyle,
};

let btn_download = KeyboardButton::builder()
    .text("دانلود")
    .style(ButtonStyle::Primary)
    .build();

let btn_delete = KeyboardButton::builder()
    .text("حذف")
    .style(ButtonStyle::Danger)
    .build();

let btn_done = KeyboardButton::builder()
    .text("انجام شد")
    .style(ButtonStyle::Success)
    .build();

let keyboard = ReplyKeyboardMarkup::builder()
    .keyboard(vec![
        vec![btn_download],
        vec![btn_done, btn_delete],
    ])
    .resize_keyboard(true)
    .build();

let params = SendMessageParams::builder()
    .chat_id(chat_id)
    .text("چیکار می‌خوای؟")
    .reply_markup(ReplyMarkup::ReplyKeyboardMarkup(keyboard))
    .build();

api.send_message(&params).await?;
```

---

## 5. Custom Emoji روی دکمه

هم `InlineKeyboardButton` هم `KeyboardButton` فیلد  
`icon_custom_emoji_id` دارن — ایموجی کنار متن دکمه نشون داده میشه:

### InlineKeyboard با custom emoji:

```rust
let btn = InlineKeyboardButton::builder()
    .text("دانلود ویدیو")
    .callback_data("download")
    .style(ButtonStyle::Primary)
    .icon_custom_emoji_id("EMOJI_ID".to_string())
    .build();
```

### ReplyKeyboard با custom emoji:

```rust
let btn = KeyboardButton::builder()
    .text("دانلود")
    .style(ButtonStyle::Success)
    .icon_custom_emoji_id("EMOJI_ID".to_string())
    .build();
```

### ترکیب هر دو (رنگ + ایموجی):

```rust
let btn = InlineKeyboardButton::builder()
    .text("تایید پرداخت")
    .callback_data("pay_confirm")
    .style(ButtonStyle::Success)          // رنگ سبز
    .icon_custom_emoji_id("EMOJI_ID".to_string())  // آیکون
    .build();
```

---

## 6. پیدا کردن Emoji ID

### روش ۱ — از sticker pack:

```rust
use frankenstein::GetStickerSetParams;

let params = GetStickerSetParams::builder()
    .name("HotCherry")  // اسم پک
    .build();

let set = api.get_sticker_set(&params).await?.result;

for sticker in &set.stickers {
    if let Some(emoji_id) = &sticker.custom_emoji_id {
        println!("emoji: {} | id: {}", 
                 sticker.emoji.as_deref().unwrap_or("?"),
                 emoji_id);
    }
}
```

### روش ۲ — resolve از ID مستقیم:

```rust
use frankenstein::GetCustomEmojiStickersParams;

let params = GetCustomEmojiStickersParams::builder()
    .custom_emoji_ids(vec!["EMOJI_ID".to_string()])
    .build();

let stickers = api.get_custom_emoji_stickers(&params).await?.result;
```

### روش ۳ — از پیام یوزر:

وقتی یوزر پریمیوم یه custom emoji بفرسته، توی `entities` پیامش  
`type: CustomEmoji` و `custom_emoji_id` هست — لاگش کن:

```rust
if let Some(entities) = &message.entities {
    for entity in entities {
        if entity.type_field == MessageEntityType::CustomEmoji {
            if let Some(id) = &entity.custom_emoji_id {
                println!("emoji id: {}", id);
            }
        }
    }
}
```

---

## 7. کپی پیام با Custom Emoji

بات می‌تونه پیام حاوی custom emoji رو **بدون نیاز به Premium** کپی کنه.  
چون پیام جدید نمی‌سازه — فقط کپی می‌کنه و entities دست نخورده منتقل میشن:

```rust
use frankenstein::{CopyMessageParams, MessageEntityType};

// چک کردن وجود custom emoji
let has_custom_emoji = message.entities
    .as_ref()
    .map(|entities| entities.iter()
        .any(|e| e.type_field == MessageEntityType::CustomEmoji))
    .unwrap_or(false);

if has_custom_emoji {
    let params = CopyMessageParams::builder()
        .chat_id(target_chat_id)      // مقصد
        .from_chat_id(message.chat.id) // مبدا
        .message_id(message.message_id)
        .build();

    api.copy_message(&params).await?;
}
```

> **چرا کار می‌کنه؟** بات پیام جدید نمی‌سازه — فقط واسطه‌ست.  
> Premium فقط برای **ساختن** custom emoji از صفر لازمه.

---

## 8. گرفتن کل پک از یه ایموجی

یوزر یه ایموجی می‌فرسته → بات همه ID های اون پک رو پیدا می‌کنه.  
**سه مرحله:**

```
custom_emoji_id → getCustomEmojiStickers → set_name → getStickerSet → همه ایموجی‌ها
```

```rust
use frankenstein::{
    AsyncTelegramApi, AsyncApi,
    GetCustomEmojiStickersParams,
    GetStickerSetParams,
    MessageEntityType,
    Message,
};

async fn get_all_emojis_from_pack(
    api: &AsyncApi,
    message: &Message,
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {

    // مرحله ۱ — custom_emoji_id رو از پیام بگیر
    let emoji_id = message.entities
        .as_ref()
        .and_then(|entities| entities.iter()
            .find(|e| e.type_field == MessageEntityType::CustomEmoji)
            .and_then(|e| e.custom_emoji_id.clone()))
        .ok_or("پیام custom emoji نداره")?;

    println!("emoji id پیدا شد: {}", emoji_id);

    // مرحله ۲ — set_name رو پیدا کن
    let params = GetCustomEmojiStickersParams::builder()
        .custom_emoji_ids(vec![emoji_id])
        .build();

    let stickers = api.get_custom_emoji_stickers(&params).await?.result;

    let set_name = stickers.first()
        .and_then(|s| s.set_name.clone())
        .ok_or("این ایموجی به پکی تعلق نداره")?;

    println!("پک پیدا شد: {}", set_name);

    // مرحله ۳ — همه ایموجی‌های پک رو بگیر
    let params = GetStickerSetParams::builder()
        .name(set_name)
        .build();

    let set = api.get_sticker_set(&params).await?.result;

    let all_emojis: Vec<(String, String)> = set.stickers
        .iter()
        .filter_map(|s| {
            let emoji_id = s.custom_emoji_id.clone()?;
            let emoji = s.emoji.clone().unwrap_or("?".to_string());
            Some((emoji_id, emoji))
        })
        .collect();

    println!("تعداد ایموجی‌های پک: {}", all_emojis.len());

    Ok(all_emojis)
}
```

### استفاده:

```rust
match get_all_emojis_from_pack(&api, &message).await {
    Ok(emojis) => {
        for (id, emoji) in &emojis {
            println!("{} → {}", emoji, id);
        }
        // حالا همه ID ها رو داری — ذخیره کن یا استفاده کن
    }
    Err(e) => eprintln!("خطا: {}", e),
}
```

### خروجی نمونه:

```
emoji id پیدا شد: 12345678901234567890
پک پیدا شد: HotCherry
تعداد ایموجی‌های پک: 47
🔥 → 12345678901234567890
❤️ → 98765432109876543210
✨ → 11223344556677889900
...
```

> **نکته:** `getCustomEmojiStickers` تا 200 ID در یه call می‌گیره.  
> `set_name` ممکنه `None` باشه اگه ایموجی به پک عمومی تعلق نداشته باشه.

---

## 9. محدودیت‌ها

| موضوع | جزئیات |
|-------|---------|
| شرط ارسال | owner بات باید Telegram Premium داشته باشه |
| دیدن توسط یوزر | همه یوزرها (حتی non-premium) ایموجی انیمیتد رو می‌بینن |
| اضافه شد | Bot API 9.4 — فوریه ۲۰۲۶ |
| offset محاسبه | UTF-16 code units (نه byte، نه character) |
| length | معمولاً ۱ — مگه surrogate pair باشه (۲) |
| style بدون Premium | style کار می‌کنه — فقط icon_custom_emoji_id نیاز به Premium داره |

### محاسبه offset:

```rust
fn utf16_offset(text: &str, target_char_index: usize) -> u16 {
    text.chars()
        .take(target_char_index)
        .map(|c| c.len_utf16() as u16)
        .sum()
}

let text = "سلام 👋 دنیا";
let offset = utf16_offset(text, 6); // موقعیت 👋
```

---

## 10. جدول مقایسه

| مکان استفاده | روش | فیلد | نیاز به Premium |
|-------------|-----|------|-----------------|
| متن پیام | `entities` array | `MessageEntity { type: CustomEmoji, custom_emoji_id }` | بله |
| caption عکس/ویدیو | `caption_entities` array | همون MessageEntity | بله |
| InlineKeyboardButton | فیلد مستقیم | `icon_custom_emoji_id: Option<String>` | بله |
| KeyboardButton | فیلد مستقیم | `icon_custom_emoji_id: Option<String>` | بله |
| رنگ InlineKeyboard | فیلد مستقیم | `style: Option<ButtonStyle>` | **خیر** |
| رنگ KeyboardButton | فیلد مستقیم | `style: Option<ButtonStyle>` | **خیر** |

> **نکته مهم:** `style` (رنگ دکمه) نیاز به Premium **ندارد** —  
> فقط `icon_custom_emoji_id` نیاز به Premium دارد.

---

> **منابع:**
> - Bot API Docs: https://core.telegram.org/bots/api#inlinekeyboardbutton
> - Bot API Docs: https://core.telegram.org/bots/api#keyboardbutton
> - Bot API Docs: https://core.telegram.org/bots/api#messageentity
> - frankenstein v0.50.0: https://github.com/ayrat555/frankenstein
> - اضافه شده در: Bot API 9.4 (February 9, 2026)
