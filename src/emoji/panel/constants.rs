pub const CB_ADD: &str = "emoji:add";
pub const CB_TEST: &str = "emoji:test";
pub const CB_LIST: &str = "emoji:list";
pub const CB_DELETE_PACK_MENU: &str = "emoji:delpack";
pub const CB_PACKS: &str = "emoji:packs";
pub const CB_IMPORT: &str = "emoji:import";
pub const CB_EXPORT: &str = "emoji:export";
pub const CB_BACK: &str = "emoji:back";
pub const CB_CANCEL: &str = "emoji:cancel";
pub const CB_PACK_OPEN_PREFIX: &str = "emoji:pack:";
pub const CB_PACK_SET_DEFAULT_PREFIX: &str = "emoji:setdef:";
pub const CB_PACK_SET_ALIAS_PREFIX: &str = "emoji:setalias:";
pub const CB_PACK_DELETE_PREFIX: &str = "emoji:packdel:";
pub const CB_LIST_PAGE_PREFIX: &str = "emoji:listpg:";
pub const CB_PICK_PACK_PREFIX: &str = "emoji:pickpack:";
pub const CB_IMPORT_REPLACE: &str = "emoji:import:replace";
pub const CB_IMPORT_MERGE: &str = "emoji:import:merge";
pub const CB_IMPORT_SMART: &str = "emoji:import:smart";
pub const CB_SHOW_PACK_LINKS: &str = "emoji:packlinks";
pub const CB_BACK_TO_PACK_CHOICE: &str = "emoji:backpick";
pub const CB_PENDING_PAGE_PREFIX: &str = "emoji:pendpg:";

pub const LIST_PAGE_SIZE: usize = 15;
pub const PENDING_PAGE_SIZE: usize = 30;

pub fn pending_total_pages(count: usize) -> usize {
    if count == 0 { 1 } else { (count + PENDING_PAGE_SIZE - 1) / PENDING_PAGE_SIZE }
}
