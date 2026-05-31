# frankenstein — Telegram Bot API Reference for AI Agents

> **frankenstein v0.50.0** | Telegram Bot API **10.0** (May 8, 2026)  
> A type-safe, low-level Rust wrapper — 1:1 mapping with the official Bot API.  
> Crate: https://crates.io/crates/frankenstein | Docs: https://docs.rs/frankenstein

---

## 📋 فهرست راهنما (Agent: اول اینجا بخون، بعد برو بخش مربوطه)

| نیاز | بخش |
|------|-----|
| راه‌اندازی اولیه، Cargo.toml، ساخت client | [§1 Setup](#1-setup--dependencies) |
| Pattern اصلی API call، async/sync | [§2 Basic Pattern](#2-basic-pattern) |
| ارسال متن، عکس، ویدیو، فایل، صدا | [§3 Sending Messages](#3-sending-messages) |
| Inline keyboard، Reply keyboard | [§4 Keyboards](#4-keyboards) |
| آپلود فایل، دریافت فایل | [§5 File Upload--Download](#5-file-uploaddownload) |
| Webhook و Long Polling | [§6 Webhook vs Long Polling](#6-webhook-vs-long-polling) |
| مدیریت گروه: ban، restrict، promote | [§7 Chat Management](#7-chat-management) |
| پرداخت، Telegram Stars | [§8 Payments--Stars](#8-paymentsstars) |
| Error handling، کدهای خطا، retry | [§9 Error Handling](#9-error-handling) |
| Business API | [§10 Business API](#10-business-api) |
| Stories | [§11 Stories](#11-stories) |
| نکات مهم: rate limit، file size، thread | [§12 Important Notes](#12-important-notes) |

---

## 1. Setup & Dependencies

```toml
# Cargo.toml
[dependencies]
frankenstein = { version = "0.50", features = ["async-http-client"] }
tokio = { version = "1", features = ["full"] }

# برای sync (بدون async):
# frankenstein = { version = "0.50", features = ["ureq"] }
```

```rust
use frankenstein::{AsyncTelegramApi, AsyncApi, SendMessageParams};

#[tokio::main]
async fn main() {
    let api = AsyncApi::new("BOT_TOKEN");
}
```

---

## 2. Basic Pattern

همه methodها یه `Params` struct می‌گیرن و `Result<MethodResponse<T>, Error>` برمی‌گردونن.

```rust
// ساختار کلی هر API call:
let params = MethodNameParams::builder()
    .required_field(value)
    .optional_field(value)   // optional fieldها با Option<T> هستن
    .build();

let result = api.method_name(&params).await?;
let data = result.result; // T مورد نظر اینجاست
```

> **نکته برای agent:** هر method یه `*Params` struct داره با همون اسم. مثلاً `sendMessage` → `SendMessageParams`.

---

## 3. Sending Messages

### sendMessage
```rust
use frankenstein::{AsyncTelegramApi, AsyncApi, SendMessageParams, ParseMode};

let params = SendMessageParams::builder()
    .chat_id(chat_id)           // i64 یا String (username)
    .text("Hello!")
    .parse_mode(ParseMode::Html) // اختیاری: Html یا MarkdownV2
    .build();

let msg = api.send_message(&params).await?.result;
println!("Message id: {}", msg.message_id);
```

### sendPhoto
```rust
use frankenstein::{SendPhotoParams, InputFile};

let params = SendPhotoParams::builder()
    .chat_id(chat_id)
    .photo(InputFile::String("file_id_or_url".into()))
    .caption("توضیح عکس")
    .build();

api.send_photo(&params).await?;
```

### sendVideo
```rust
use frankenstein::SendVideoParams;

let params = SendVideoParams::builder()
    .chat_id(chat_id)
    .video(InputFile::String("file_id".into()))
    .build();

api.send_video(&params).await?;
```

### sendAudio
```rust
let params = SendAudioParams::builder()
    .chat_id(chat_id)
    .audio(InputFile::String("file_id".into()))
    .build();

api.send_audio(&params).await?;
```

### sendDocument
```rust
let params = SendDocumentParams::builder()
    .chat_id(chat_id)
    .document(InputFile::String("file_id".into()))
    .build();

api.send_document(&params).await?;
```

### sendAnimation (GIF)
```rust
let params = SendAnimationParams::builder()
    .chat_id(chat_id)
    .animation(InputFile::String("file_id".into()))
    .build();

api.send_animation(&params).await?;
```

### sendVoice
```rust
let params = SendVoiceParams::builder()
    .chat_id(chat_id)
    .voice(InputFile::String("file_id".into()))
    .build();

api.send_voice(&params).await?;
```

### sendMediaGroup
```rust
use frankenstein::{SendMediaGroupParams, InputMedia, InputMediaPhoto};

let media = vec![
    InputMedia::Photo(InputMediaPhoto::builder()
        .media(InputFile::String("file_id_1".into()))
        .build()),
    InputMedia::Photo(InputMediaPhoto::builder()
        .media(InputFile::String("file_id_2".into()))
        .build()),
];

let params = SendMediaGroupParams::builder()
    .chat_id(chat_id)
    .media(media)
    .build();

api.send_media_group(&params).await?;
```

### sendChatAction
```rust
use frankenstein::{SendChatActionParams, ChatAction};

let params = SendChatActionParams::builder()
    .chat_id(chat_id)
    .action(ChatAction::Typing)  // Typing, UploadPhoto, RecordVideo, ...
    .build();

api.send_chat_action(&params).await?;
```

---

## 4. Keyboards

### Inline Keyboard
```rust
use frankenstein::{
    SendMessageParams, InlineKeyboardMarkup, InlineKeyboardButton, ReplyMarkup
};

let button1 = InlineKeyboardButton::builder()
    .text("دکمه ۱")
    .callback_data("btn_1")
    .build();

let button2 = InlineKeyboardButton::builder()
    .text("لینک")
    .url("https://example.com")
    .build();

let keyboard = InlineKeyboardMarkup::builder()
    .inline_keyboard(vec![
        vec![button1, button2],  // هر vec یه ردیفه
    ])
    .build();

let params = SendMessageParams::builder()
    .chat_id(chat_id)
    .text("یه keyboard:")
    .reply_markup(ReplyMarkup::InlineKeyboardMarkup(keyboard))
    .build();

api.send_message(&params).await?;
```

### پاسخ به Callback Query
```rust
use frankenstein::AnswerCallbackQueryParams;

let params = AnswerCallbackQueryParams::builder()
    .callback_query_id("CALLBACK_ID")
    .text("کلیک شد!")   // اختیاری: پیام popup
    .build();

api.answer_callback_query(&params).await?;
```

### Reply Keyboard
```rust
use frankenstein::{ReplyKeyboardMarkup, KeyboardButton, ReplyMarkup};

let keyboard = ReplyKeyboardMarkup::builder()
    .keyboard(vec![
        vec![
            KeyboardButton::builder().text("گزینه ۱").build(),
            KeyboardButton::builder().text("گزینه ۲").build(),
        ],
    ])
    .resize_keyboard(true)
    .build();

let params = SendMessageParams::builder()
    .chat_id(chat_id)
    .text("انتخاب کنید:")
    .reply_markup(ReplyMarkup::ReplyKeyboardMarkup(keyboard))
    .build();

api.send_message(&params).await?;
```

### حذف Keyboard
```rust
use frankenstein::{ReplyKeyboardRemove, ReplyMarkup};

let remove = ReplyKeyboardRemove::builder().remove_keyboard(true).build();

let params = SendMessageParams::builder()
    .chat_id(chat_id)
    .text("keyboard حذف شد")
    .reply_markup(ReplyMarkup::ReplyKeyboardRemove(remove))
    .build();

api.send_message(&params).await?;
```

---

## 5. File Upload/Download

### آپلود فایل از disk
```rust
use std::path::PathBuf;
use frankenstein::{SendDocumentParams, InputFile};

let params = SendDocumentParams::builder()
    .chat_id(chat_id)
    .document(InputFile::InputFile {
        path: PathBuf::from("/path/to/file.pdf")
    })
    .build();

api.send_document(&params).await?;
```

### دریافت اطلاعات فایل
```rust
use frankenstein::GetFileParams;

let params = GetFileParams::builder()
    .file_id("FILE_ID")
    .build();

let file = api.get_file(&params).await?.result;
// file.file_path → مسیر برای دانلود
// دانلود: https://api.telegram.org/file/bot{TOKEN}/{file_path}
// حداکثر حجم: 20 MB
```

### آپلود sticker
```rust
use frankenstein::{UploadStickerFileParams, StickerFormat, InputFile};

let params = UploadStickerFileParams::builder()
    .user_id(user_id)
    .sticker(InputFile::InputFile { path: PathBuf::from("sticker.webp") })
    .sticker_format(StickerFormat::Static)
    .build();

let file = api.upload_sticker_file(&params).await?.result;
```

> **محدودیت‌های فایل:**
> - sendDocument: تا 50 MB
> - getFile (دانلود): تا 20 MB
> - local Bot API server: تا 2000 MB

---

## 6. Webhook vs Long Polling

### Long Polling (برای توسعه)
```rust
use frankenstein::{GetUpdatesParams, UpdateContent};

let mut params = GetUpdatesParams::builder()
    .timeout(30u32)
    .build();

loop {
    let updates = api.get_updates(&params).await?.result;
    
    for update in updates {
        // پردازش update
        match update.content {
            UpdateContent::Message(msg) => { /* ... */ }
            UpdateContent::CallbackQuery(cb) => { /* ... */ }
            _ => {}
        }
        
        // offset رو آپدیت کن
        params.offset = Some((update.update_id + 1) as i64);
    }
}
```

### Webhook (برای production)
```rust
use frankenstein::SetWebhookParams;

// فعال کردن webhook
let params = SetWebhookParams::builder()
    .url("https://yourdomain.com/webhook")
    .build();

api.set_webhook(&params).await?;

// حذف webhook (برگشت به long polling)
api.delete_webhook(&Default::default()).await?;

// وضعیت webhook
let info = api.get_webhook_info().await?.result;
```

> **قانون مهم:** نمی‌شه هم‌زمان از webhook و getUpdates استفاده کرد.

---

## 7. Chat Management

### Ban / Unban
```rust
use frankenstein::{BanChatMemberParams, UnbanChatMemberParams};

// ban
let params = BanChatMemberParams::builder()
    .chat_id(chat_id)
    .user_id(user_id)
    .build();
api.ban_chat_member(&params).await?;

// unban
let params = UnbanChatMemberParams::builder()
    .chat_id(chat_id)
    .user_id(user_id)
    .build();
api.unban_chat_member(&params).await?;
```

### Restrict (محدود کردن)
```rust
use frankenstein::{RestrictChatMemberParams, ChatPermissions};

let permissions = ChatPermissions::builder()
    .can_send_messages(false)
    .can_send_media_messages(false)
    .build();

let params = RestrictChatMemberParams::builder()
    .chat_id(chat_id)
    .user_id(user_id)
    .permissions(permissions)
    .build();

api.restrict_chat_member(&params).await?;
```

### Promote (ادمین کردن)
```rust
use frankenstein::PromoteChatMemberParams;

let params = PromoteChatMemberParams::builder()
    .chat_id(chat_id)
    .user_id(user_id)
    .can_delete_messages(true)
    .can_restrict_members(true)
    .build();

api.promote_chat_member(&params).await?;
```

### اطلاعات عضو
```rust
use frankenstein::{GetChatMemberParams, GetChatAdministratorsParams};

// یه عضو خاص
let params = GetChatMemberParams::builder()
    .chat_id(chat_id)
    .user_id(user_id)
    .build();
let member = api.get_chat_member(&params).await?.result;

// همه ادمین‌ها
let params = GetChatAdministratorsParams::builder()
    .chat_id(chat_id)
    .build();
let admins = api.get_chat_administrators(&params).await?.result;
```

---

## 8. Payments/Stars

### ارسال فاکتور
```rust
use frankenstein::{SendInvoiceParams, LabeledPrice};

let prices = vec![
    LabeledPrice::builder()
        .label("محصول")
        .amount(500)  // به کوچکترین واحد ارز
        .build()
];

let params = SendInvoiceParams::builder()
    .chat_id(chat_id)
    .title("نام محصول")
    .description("توضیحات")
    .payload("unique_payload")
    .currency("XTR")   // XTR = Telegram Stars
    .prices(prices)
    .build();

api.send_invoice(&params).await?;
```

### تأیید پرداخت
```rust
use frankenstein::AnswerPreCheckoutQueryParams;

// باید ظرف 10 ثانیه پاسخ بدی!
let params = AnswerPreCheckoutQueryParams::builder()
    .pre_checkout_query_id("QUERY_ID")
    .ok(true)
    .build();

api.answer_pre_checkout_query(&params).await?;
```

### Stars
```rust
// موجودی Stars بات
let balance = api.get_my_star_balance().await?.result;

// تراکنش‌ها
use frankenstein::GetStarTransactionsParams;
let params = GetStarTransactionsParams::builder().build();
let transactions = api.get_star_transactions(&params).await?.result;

// استرداد
use frankenstein::RefundStarPaymentParams;
let params = RefundStarPaymentParams::builder()
    .user_id(user_id)
    .telegram_payment_charge_id("CHARGE_ID")
    .build();
api.refund_star_payment(&params).await?;
```

---

## 9. Error Handling

```rust
use frankenstein::Error;

match api.send_message(&params).await {
    Ok(response) => {
        let msg = response.result;
    }
    Err(Error::Api(api_err)) => {
        match api_err.error_code {
            400 => eprintln!("Bad Request: {}", api_err.description),
            401 => eprintln!("Unauthorized: توکن اشتباهه"),
            403 => eprintln!("Forbidden: بات بلاک شده یا دسترسی نداره"),
            429 => {
                // Rate limit — باید صبر کنی
                let retry_after = api_err.parameters
                    .and_then(|p| p.retry_after)
                    .unwrap_or(1);
                eprintln!("Rate limited! retry after {} seconds", retry_after);
                tokio::time::sleep(Duration::from_secs(retry_after)).await;
            }
            500 => eprintln!("Telegram server error"),
            _ => eprintln!("Error {}: {}", api_err.error_code, api_err.description),
        }
    }
    Err(Error::Http(e)) => eprintln!("HTTP error: {}", e),
    Err(e) => eprintln!("Other error: {}", e),
}
```

> **نکته مهم:** همیشه فیلد `description` رو لاگ کن — جزئیات دقیق خطا اونجاست.

---

## 10. Business API

```rust
use frankenstein::{GetBusinessConnectionParams, ReadBusinessMessageParams};

// اطلاعات business connection
let params = GetBusinessConnectionParams::builder()
    .business_connection_id("CONNECTION_ID")
    .build();
let connection = api.get_business_connection(&params).await?.result;

// خواندن پیام
let params = ReadBusinessMessageParams::builder()
    .business_connection_id("CONNECTION_ID")
    .chat_id(chat_id)
    .message_id(msg_id)
    .build();
api.read_business_message(&params).await?;

// تغییر نام اکانت business
use frankenstein::SetBusinessAccountNameParams;
let params = SetBusinessAccountNameParams::builder()
    .business_connection_id("CONNECTION_ID")
    .first_name("نام جدید")
    .build();
api.set_business_account_name(&params).await?;
```

> **نکته:** همه methodهای Business API نیاز به `business_connection_id` دارن.

---

## 11. Stories

> فقط برای بات‌هایی که Business Account مدیریت می‌کنن.  
> نیاز به `can_manage_stories` در business connection.

```rust
use frankenstein::{PostStoryParams, InputStoryContent, InputStoryContentPhoto};

// پست کردن story
let content = InputStoryContent::Photo(
    InputStoryContentPhoto::builder()
        .photo(InputFile::String("file_id".into()))
        .build()
);

let params = PostStoryParams::builder()
    .business_connection_id("CONNECTION_ID")
    .content(content)
    .active_period(86400)  // ثانیه (86400 = 24 ساعت)
    .build();

let story = api.post_story(&params).await?.result;

// حذف story
use frankenstein::DeleteStoryParams;
let params = DeleteStoryParams::builder()
    .business_connection_id("CONNECTION_ID")
    .story_id(story.id)
    .build();
api.delete_story(&params).await?;
```

---

## 12. Important Notes

### Rate Limits
- **30 پیام در ثانیه** برای همه chat‌ها
- **1 پیام در ثانیه** برای هر chat خاص
- **20 پیام در دقیقه** به یه گروه
- خطای 429 → صبر کن به اندازه `retry_after`

### Async vs Sync
```toml
# Async (توصیه شده):
frankenstein = { version = "0.50", features = ["async-http-client"] }
# استفاده: AsyncApi, AsyncTelegramApi

# Sync:
frankenstein = { version = "0.50", features = ["ureq"] }
# استفاده: Api, TelegramApi
```

### chat_id
```rust
// می‌تونه عدد (i64) یا username (String) باشه:
.chat_id(123456789_i64)
.chat_id("@channelname".to_string())
```

### business_connection_id
از Bot API 10.0، خیلی از methodهای send پارامتر اختیاری `business_connection_id` دارن — برای ارسال از طرف یه Business Account.

### Thread / Forum
پارامتر اختیاری `message_thread_id` برای ارسال در یه topic خاص از Forum گروه‌ها.

### محدودیت حجم فایل
| روش | حداکثر |
|-----|--------|
| sendDocument / sendVideo | 50 MB |
| getFile (دانلود) | 20 MB |
| local Bot API server | 2000 MB |

---

> **منابع:**
> - Official Bot API Docs: https://core.telegram.org/bots/api
> - frankenstein Docs: https://docs.rs/frankenstein/0.50.0
> - frankenstein GitHub: https://github.com/ayrat555/frankenstein
