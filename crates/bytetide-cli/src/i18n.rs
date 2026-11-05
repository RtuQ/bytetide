//! CLI language selection from the process locale.

pub fn english() -> bool {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|name| std::env::var(name).ok())
        .find(|value| !value.is_empty())
        .is_some_and(|value| !value.to_ascii_lowercase().starts_with("zh"))
}

pub fn text(zh: &'static str, en: &'static str) -> &'static str {
    if english() {
        en
    } else {
        zh
    }
}
