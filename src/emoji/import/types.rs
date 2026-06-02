#[derive(Debug, Clone)]
pub struct ParsedPack {
    pub old_id: i32,
    pub name: String,
    pub alias: Option<String>,
    pub is_default: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedItem {
    pub old_pack_id: i32,
    pub custom_emoji_id: String,
    pub fallback: String,
    pub smart_name: String,
    pub alias: Option<String>,
    pub position: i32,
}

#[derive(Debug, Default, Clone)]
pub struct ParsedSql {
    pub packs: Vec<ParsedPack>,
    pub items: Vec<ParsedItem>,
}

#[derive(Debug)]
pub struct ImportAnalysis {
    pub file_packs: usize,
    pub file_items: usize,
    pub db_packs: usize,
    pub db_items: usize,
    pub duplicate_items: usize,
    pub db_empty: bool,
}

#[derive(Debug)]
pub struct ImportResult {
    pub packs_added: usize,
    pub items_added: usize,
    pub items_skipped: usize,
}
