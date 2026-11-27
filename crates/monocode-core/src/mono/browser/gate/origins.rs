//! The Mono's origin rules: which pages a Mono may read, from its blocked and allowed lists.
//! The Ardia upstream checks them before it opens or moves a tab and after it reads a page.

/// Which pages a Mono may read: the Mono's blocked and allowed origin lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Origins {
    allowed: Vec<String>,
    blocked: Vec<String>,
}

/// Why a page is not read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadRefusal {
    /// A browser page, a file or another address that is not http or https.
    Restricted,
    Blocked,
    /// The Mono has an allowed list and the page is not on it.
    NotAllowed,
}

impl Origins {
    /// Lists as saved in the Mono's browser setting. Entries that are not origins are dropped.
    pub fn new(allowed: &[String], blocked: &[String]) -> Self {
        let clean = |list: &[String]| {
            list.iter()
                .filter_map(|o| super::super::normalize_origin(o).ok())
                .collect()
        };
        Self {
            allowed: clean(allowed),
            blocked: clean(blocked),
        }
    }
    /// `Ok` when the page at `url` may be read.
    pub fn check(&self, url: &str) -> Result<(), ReadRefusal> {
        let Some(origin) = origin_of(url) else {
            // An empty page is harmless and a new tab starts as one; every other address that has
            // no web origin is a browser page.
            return if url == "about:blank" {
                Ok(())
            } else {
                Err(ReadRefusal::Restricted)
            };
        };
        if is_web_store(url) {
            return Err(ReadRefusal::Restricted);
        }
        if self.blocked.contains(&origin) {
            return Err(ReadRefusal::Blocked);
        }
        if !self.allowed.is_empty() && !self.allowed.contains(&origin) {
            return Err(ReadRefusal::NotAllowed);
        }
        Ok(())
    }
}

/// `scheme://host[:port]` of an http or https address.
pub fn origin_of(url: &str) -> Option<String> {
    let parsed = url::Url::parse(url.trim()).ok()?;
    matches!(parsed.scheme(), "http" | "https").then_some(())?;
    parsed.host_str()?;
    Some(parsed.origin().ascii_serialization())
}

/// The browser extension stores (Chrome Web Store, Edge Add-ons) are never read.
fn is_web_store(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let host = parsed.host_str().unwrap_or("");
    let path = parsed.path();
    host == "chromewebstore.google.com"
        || (host == "chrome.google.com" && path.starts_with("/webstore"))
        || (host == "microsoftedge.microsoft.com" && path.starts_with("/addons"))
}

/// The sentence the Mono gets when [`Origins::check`] refuses a page.
pub(crate) fn refusal_text(refusal: ReadRefusal) -> &'static str {
    match refusal {
        ReadRefusal::Restricted => "This is a browser page (or a file): zptah never reads it.",
        ReadRefusal::Blocked => "This site is on this Mono's blocked list, so zptah does not read it.",
        ReadRefusal::NotAllowed => "This Mono may only use the sites on its allowed list, and this page is not one of them.",
    }
}

/// Whether a text is Vietnamese (a letter only Vietnamese uses): the lexicon reads some words
/// differently on a Vietnamese page.
pub(crate) fn looks_vietnamese(text: &str) -> bool {
    const ONLY_VI: &str = "ăâđêôơưĂÂĐÊÔƠƯạảãấầẩẫậắằẳẵặẹẻẽếềểễệỉịọỏốồổỗộớờởỡợụủứừửữựỳỵỷỹẠẢÃẤẦẨẪẬẮẰẲẴẶẸẺẼẾỀỂỄỆỈỊỌỎỐỒỔỖỘỚỜỞỠỢỤỦỨỪỬỮỰỲỴỶỸ";
    text.chars().any(|c| ONLY_VI.contains(c))
}
