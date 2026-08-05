# راهنمای کامل MarkdownV2 در Telegram Bot API 10.1+ با فریم‌ورک Frankenstein (Rust)

> **نسخه هدف:** Telegram Bot API **10.1** و بالاتر  
> **فریم‌ورک:** [frankenstein](https://github.com/ayrat555/frankenstein) (آخرین نسخه‌ها از API 10.1 و 10.2 پشتیبانی کامل دارند)  
> **تاریخ تهیه:** جولای ۲۰۲۶

---

## فهرست مطالب

1. [مقدمه](#مقدمه)
2. [MarkdownV2 چیست؟](#markdownv2-چیست)
3. [کاراکترهای خاص و قوانین Escape](#کاراکترهای-خاص-و-قوانین-escape)
4. [توابع کمکی Escape در Rust](#توابع-کمکی-escape-در-rust)
5. [ارسال پیام با MarkdownV2](#ارسال-پیام-با-markdownv2)
6. [مثال‌های کامل](#مثال‌های-کامل)
7. [نکات پیشرفته و اشتباهات رایج](#نکات-پیشرفته-و-اشتباهات-رایج)
8. [ارتباط با Rich Messages (Bot API 10.1)](#ارتباط-با-rich-messages-bot-api-101)
9. [منابع](#منابع)

---

## مقدمه

از نسخهٔ Bot API 4.0 به بعد، تلگرام دو حالت فرمت‌بندی متن اصلی را معرفی کرد:

- **Markdown** (قدیمی و منسوخ)
- **MarkdownV2** (توصیه‌شده)
- **HTML**

**MarkdownV2** دقیق‌تر، امن‌تر و قدرتمندتر از Markdown قدیمی است و تقریباً تمام ویژگی‌های فرمت‌بندی مدرن (bold, italic, underline, strikethrough, spoiler, code, pre, link, mention, custom emoji و ...) را پشتیبانی می‌کند.

فریم‌ورک **Frankenstein** یک کلاینت کامل و type-safe برای Telegram Bot API در زبان Rust است که ساختارها را یک‌به‌یک از مستندات رسمی تلگرام نگاشت می‌کند و از نسخه‌های جدید API (از جمله 10.1 و 10.2) پشتیبانی کامل دارد.

---

## MarkdownV2 چیست؟

برای استفاده از MarkdownV2 کافی است پارامتر `parse_mode` را برابر با `"MarkdownV2"` قرار دهید.

### سینتکس اصلی

| فرمت                  | سینتکس MarkdownV2                          | توضیح |
|-----------------------|---------------------------------------------|-------|
| **Bold**              | `*bold text*`                               | متن ضخیم |
| *Italic*              | `_italic text_`                             | متن مورب |
| Underline             | `__underline__`                             | زیرخط |
| ~~Strikethrough~~     | `~strikethrough~`                           | خط خورده |
| \|\|Spoiler\|\|       | `\|\|spoiler\|\|`                           | اسپویلر |
| `Inline code`         | `` `inline code` ``                         | کد درون‌خطی |
| Code block            | ```` ``` ```` + کد + ```` ``` ````         | بلوک کد |
| Code block با زبان    | ```` ```rust ```` + کد + ```` ``` ````     | بلوک کد با هایلایت |
| [Link](url)           | `[text](https://example.com)`               | لینک |
| Mention کاربر         | `[User](tg://user?id=123456789)`            | منشن کاربر |
| Custom Emoji          | `![👍](tg://emoji?id=5368324170671202286)`  | ایموجی سفارشی |
| Blockquote            | `> quote`                                   | نقل‌قول |
| Expandable Blockquote | `**> expandable quote`                      | نقل‌قول قابل گسترش |

**نکته مهم:** کاراکترهای خاص باید escape شوند، در غیر این صورت پیام ارسال نمی‌شود و خطای `Bad Request: can't parse entities` دریافت می‌کنید.

---

## کاراکترهای خاص و قوانین Escape

طبق مستندات رسمی تلگرام، در **متن معمولی** (خارج از entityها) این کاراکترها باید با `\` escape شوند:

```
_ * [ ] ( ) ~ ` > # + - = | { } . !
```

### قوانین دقیق‌تر:

1. **داخل entityها** (مثل داخل `*bold*`) نباید کاراکترهای خاص را escape کنید مگر اینکه خود entity نیاز داشته باشد.
2. برای نوشتن خود کاراکتر `*` داخل bold:
   ```
   *bold \* text*
   ```
3. برای italic با underscore در وسط:
   ```
   _snake\_case_
   ```
4. داخل **code** و **pre** فقط `` ` `` و `\` نیاز به escape دارند.
5. کاراکتر `\` خودش هم باید escape شود (`\\`).

---

## توابع کمکی Escape در Rust

یک تابع کامل و production-ready برای escape کردن متن در MarkdownV2:

```rust
/// Escape special characters for Telegram MarkdownV2.
/// Use this for plain text parts of your message.
pub fn escape_markdown_v2(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        match c {
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|' | '{' | '}' | '.' | '!' | '\\' => {
                result.push('\\');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result
}

/// Escape only for use inside code / pre blocks.
pub fn escape_markdown_v2_code(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    for c in text.chars() {
        match c {
            '`' | '\\' => {
                result.push('\\');
                result.push(c);
            }
            _ => result.push(c),
        }
    }
    result
}
```

### نسخهٔ پیشرفته‌تر (با پشتیبانی از nesting ساده)

```rust
use std::fmt::Write;

pub struct MarkdownV2;

impl MarkdownV2 {
    pub fn escape(text: &str) -> String {
        escape_markdown_v2(text)
    }

    pub fn bold(text: &str) -> String {
        format!("*{}*", Self::escape(text))
    }

    pub fn italic(text: &str) -> String {
        format!("_{}_", Self::escape(text))
    }

    pub fn underline(text: &str) -> String {
        format!("__{}__", Self::escape(text))
    }

    pub fn strikethrough(text: &str) -> String {
        format!("~{}~", Self::escape(text))
    }

    pub fn spoiler(text: &str) -> String {
        format!("||{}||", Self::escape(text))
    }

    pub fn code(text: &str) -> String {
        format!("`{}`", escape_markdown_v2_code(text))
    }

    pub fn pre(text: &str, language: Option<&str>) -> String {
        match language {
            Some(lang) => format!("```{}\n{}\n```", lang, escape_markdown_v2_code(text)),
            None => format!("```\n{}\n```", escape_markdown_v2_code(text)),
        }
    }

    pub fn link(text: &str, url: &str) -> String {
        format!("[{}]({})", Self::escape(text), url)
    }

    pub fn mention(text: &str, user_id: i64) -> String {
        format!("[{}](tg://user?id={})", Self::escape(text), user_id)
    }

    pub fn custom_emoji(emoji: &str, emoji_id: &str) -> String {
        format!("![{}](tg://emoji?id={})", emoji, emoji_id)
    }
}
```

---

## ارسال پیام با MarkdownV2

### Async (پیشنهادی)

```rust
use frankenstein::{
    AsyncTelegramApi,
    client_reqwest::Bot,
    methods::SendMessageParams,
    ParseMode,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = "YOUR_BOT_TOKEN";
    let bot = Bot::new(token);

    let text = format!(
        "{}\n{}\n{}",
        MarkdownV2::bold("سلام!"),
        MarkdownV2::italic("این یک پیام تست MarkdownV2 است"),
        MarkdownV2::code("println!(\"Hello Frankenstein\");")
    );

    let params = SendMessageParams::builder()
        .chat_id(123456789_i64)          // chat_id خود را بگذارید
        .text(text)
        .parse_mode(ParseMode::MarkdownV2)
        .build();

    let response = bot.send_message(&params).await?;
    println!("Message sent: {:?}", response);

    Ok(())
}
```

### Blocking

```rust
use frankenstein::{
    TelegramApi,
    client_ureq::Bot,
    methods::SendMessageParams,
    ParseMode,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = "YOUR_BOT_TOKEN";
    let bot = Bot::new(token);

    let text = "*Bold* and _Italic_ and `code`";

    let params = SendMessageParams::builder()
        .chat_id(123456789_i64)
        .text(text)
        .parse_mode(ParseMode::MarkdownV2)
        .build();

    let response = bot.send_message(&params)?;
    println!("{:?}", response);

    Ok(())
}
```

---

## مثال‌های کامل

### مثال ۱: پیام ترکیبی زیبا

```rust
let message = format!(
    "{}\n\n\
     {}\n\
     {}\n\n\
     {}\n\
     {}\n\n\
     {}",
    MarkdownV2::bold("📢 اعلان جدید"),
    MarkdownV2::italic("تاریخ: ") + &MarkdownV2::code("2026-07-25"),
    MarkdownV2::spoiler("اطلاعات محرمانه اینجا قرار می‌گیرد"),
    MarkdownV2::link("مستندات رسمی", "https://core.telegram.org/bots/api"),
    MarkdownV2::mention("کاربر تست", 123456789),
    MarkdownV2::pre(
        r#"fn main() {
    println!("Hello from Frankenstein + MarkdownV2!");
}"#,
        Some("rust")
    )
);
```

### مثال ۲: Escape خودکار متن کاربر

```rust
fn send_user_input(bot: &Bot, chat_id: i64, user_text: &str) {
    // همیشه متن ورودی کاربر را escape کنید
    let safe_text = MarkdownV2::escape(user_text);

    let text = format!(
        "{}\n{}",
        MarkdownV2::bold("متن شما:"),
        safe_text
    );

    let params = SendMessageParams::builder()
        .chat_id(chat_id)
        .text(text)
        .parse_mode(ParseMode::MarkdownV2)
        .build();

    let _ = bot.send_message(&params); // در async از .await استفاده کنید
}
```

### مثال ۳: استفاده از MessageEntity به جای parse_mode (روش جایگزین)

گاهی اوقات استفاده از `entities` امن‌تر است (مخصوصاً وقتی متن پیچیده دارید):

```rust
use frankenstein::types::{MessageEntity, MessageEntityType};

let text = "Hello bold world";
let entities = vec![
    MessageEntity {
        type_field: MessageEntityType::Bold,
        offset: 6,
        length: 4,
        url: None,
        user: None,
        language: None,
        custom_emoji_id: None,
        // فیلدهای جدید API 10.x در صورت نیاز
        ..Default::default()
    },
];

let params = SendMessageParams::builder()
    .chat_id(chat_id)
    .text(text)
    .entities(entities)
    .build();
```

---

## نکات پیشرفته و اشتباهات رایج

### ۱. خطای `can't parse entities`
معمولاً به این دلایل رخ می‌دهد:
- فراموش کردن escape یک کاراکتر خاص
- بستن نادرست entity (مثلاً `*bold` بدون `*`)
- nesting نادرست (مثل `*_bold italic_*`)

### ۲. ترتیب بستن entityها مهم است
```
*bold _italic bold* italic_     ❌ اشتباه
*bold _italic bold_ bold*       ✅ درست
```

### ۳. کاراکتر `.` و `!` هم باید escape شوند
بسیاری افراد این دو را فراموش می‌کنند.

### ۴. محدودیت طول پیام
- پیام عادی: حداکثر **4096** کاراکتر
- در Rich Messages (API 10.1+): تا **32768** کاراکتر

### ۵. استفاده همزمان با Rich Messages
از Bot API **10.1** به بعد، تلگرام سیستم جدید **Rich Messages** را معرفی کرده که قدرتمندتر از MarkdownV2 است و از بلوک‌های ساختاریافته (جدول، لیست، heading، media و ...) پشتیبانی می‌کند.  
Frankenstein ماژول `rich_message` را برای این منظور دارد.

اگر فقط نیاز به فرمت‌بندی ساده دارید، MarkdownV2 همچنان بهترین و ساده‌ترین گزینه است.

---

## ارتباط با Rich Messages (Bot API 10.1)

در Bot API 10.1 (۱۱ ژوئن ۲۰۲۶) قابلیت **Rich Messages** اضافه شد:

- پیام‌های ساختاریافته با بلوک‌های مختلف
- پشتیبانی از جدول، لیست، heading، quotation، media و ...
- امکان streaming پاسخ‌های AI
- حداکثر ۳۲۷۶۸ کاراکتر

Frankenstein از نسخهٔ مربوط به API 10.1 به بعد ماژول `rich_message` و متد `send_rich_message` را ارائه می‌دهد.

**پیشنهاد:**
- برای پیام‌های ساده و سریع → **MarkdownV2**
- برای محتوای پیچیده، لانگ‌رید، جداول و AI streaming → **Rich Messages**

---

## منابع

- [مستندات رسمی Telegram Bot API – Formatting options](https://core.telegram.org/bots/api#formatting-options)
- [Frankenstein GitHub](https://github.com/ayrat555/frankenstein)
- [docs.rs/frankenstein](https://docs.rs/frankenstein)
- [Changelog Bot API](https://core.telegram.org/bots/api-changelog)
- [crate telegram-markdown-v2](https://crates.io/crates/telegram-markdown-v2) (برای تبدیل Markdown معمولی به MarkdownV2)

---

**ساخته شده برای استفاده با Frankenstein + Bot API 10.1+**  
اگر سوال یا پیشنهادی داشتی، بگو تا فایل را آپدیت کنم.
