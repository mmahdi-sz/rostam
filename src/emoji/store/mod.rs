mod pack;
mod item;
mod export;

pub use pack::{create_pack, find_pack_by_name, list_packs, get_default_pack, set_default_pack, set_pack_alias, delete_pack};
pub use item::{list_items, existing_custom_emoji_ids, allocate_smart_name, add_item, set_item_alias};
pub use export::{export_user_sql, render_template};

#[derive(Debug, Clone)]
pub struct EmojiPack {
    pub id: i32,
    pub owner_user_id: i64,
    pub name: String,
    pub alias: Option<String>,
    pub is_default: bool,
    pub item_count: i64,
}

#[derive(Debug, Clone)]
pub struct EmojiItem {
    pub id: i32,
    pub pack_id: i32,
    pub custom_emoji_id: String,
    pub fallback: String,
    pub smart_name: String,
    pub alias: Option<String>,
    pub position: i32,
}

pub(self) fn row_to_pack(row: tokio_postgres::Row) -> EmojiPack {
    EmojiPack {
        id: row.get(0),
        owner_user_id: row.get(1),
        name: row.get(2),
        alias: row.get(3),
        is_default: row.get(4),
        item_count: row.get(5),
    }
}

pub(self) fn row_to_item(row: tokio_postgres::Row) -> EmojiItem {
    EmojiItem {
        id: row.get(0),
        pack_id: row.get(1),
        custom_emoji_id: row.get(2),
        fallback: row.get(3),
        smart_name: row.get(4),
        alias: row.get(5),
        position: row.get(6),
    }
}
