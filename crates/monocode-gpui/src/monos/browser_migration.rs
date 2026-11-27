//! The trust levels of the Browser page and their labels. The labels follow the app language like
//! the approval cards.
use monocode_core::mono::browser::BrowserLevel;
use monocode_core::mono::browser::copy::Lang;

/// The trust levels a user can choose. `Ask every` and `Autonomous` are gone from the page; a
/// stored legacy level already reads as `Act for me`.
pub(super) const CHOICES: [BrowserLevel; 2] = [BrowserLevel::ReadOnly, BrowserLevel::Act];

pub(super) fn trust_label(trust: BrowserLevel, lang: Lang) -> &'static str {
    match (trust, lang) {
        (BrowserLevel::ReadOnly, Lang::En) => "Read only",
        (BrowserLevel::ReadOnly, Lang::Vi) => "Chỉ đọc",
        (BrowserLevel::Act, Lang::En) => "Act for me",
        (BrowserLevel::Act, Lang::Vi) => "Làm giúp tôi",
    }
}
