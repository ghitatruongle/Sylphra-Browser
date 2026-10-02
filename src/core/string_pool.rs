use std::collections::HashMap;
use std::sync::Arc;

use crate::resource_caps;

#[derive(Debug, Clone, Default)]
pub struct StringPool {
    inner: HashMap<Arc<str>, Arc<str>>,
    held_bytes: usize,
    duplicate_bytes: usize,
}

impl StringPool {
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
            held_bytes: 0,
            duplicate_bytes: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: HashMap::with_capacity(capacity),
            held_bytes: 0,
            duplicate_bytes: 0,
        }
    }

    pub fn intern(&mut self, s: &str) -> Arc<str> {
        if let Some(existing) = self.inner.get(s) {
            self.duplicate_bytes = self.duplicate_bytes.saturating_add(s.len());
            return existing.clone();
        }
        if !self.accepts_new(s.len()) {
            return Arc::from(s);
        }
        let arc: Arc<str> = Arc::from(s);
        self.held_bytes = self.held_bytes.saturating_add(arc.len());
        self.inner.insert(arc.clone(), arc.clone());
        arc
    }

    fn accepts_new(&self, incoming: usize) -> bool {
        self.inner.len() < resource_caps::INTERNED_STRINGS_MAX_ENTRIES
            && self
                .held_bytes
                .saturating_add(incoming)
                .saturating_add(std::mem::size_of::<Arc<str>>() * 2)
                <= resource_caps::INTERNED_STRINGS_MAX_BYTES
    }

    pub fn intern_owned(&mut self, s: String) -> Arc<str> {
        self.intern(&s)
    }

    pub fn held_bytes(&self) -> usize {
        self.held_bytes
    }

    pub fn savings_bytes(&self) -> usize {
        self.duplicate_bytes
    }

    pub fn contains(&self, s: &str) -> bool {
        self.inner.contains_key(s)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn clear(&mut self) {
        self.inner.clear();
        self.held_bytes = 0;
        self.duplicate_bytes = 0;
    }

    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    pub fn preload_common(&mut self) {
        for tag in &[
            "div",
            "span",
            "p",
            "a",
            "img",
            "ul",
            "ol",
            "li",
            "table",
            "tr",
            "td",
            "th",
            "thead",
            "tbody",
            "tfoot",
            "form",
            "input",
            "button",
            "select",
            "option",
            "textarea",
            "label",
            "fieldset",
            "legend",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "header",
            "footer",
            "nav",
            "main",
            "section",
            "article",
            "aside",
            "figure",
            "figcaption",
            "details",
            "summary",
            "dialog",
            "menu",
            "iframe",
            "script",
            "style",
            "link",
            "meta",
            "title",
            "head",
            "body",
            "html",
            "br",
            "hr",
            "wbr",
            "source",
            "track",
            "embed",
            "object",
            "param",
            "video",
            "audio",
            "canvas",
            "map",
            "area",
            "col",
            "colgroup",
            "caption",
            "datalist",
            "optgroup",
            "output",
            "progress",
            "meter",
            "time",
            "data",
            "abbr",
            "address",
            "blockquote",
            "cite",
            "code",
            "del",
            "ins",
            "dfn",
            "em",
            "strong",
            "small",
            "s",
            "sub",
            "sup",
            "mark",
            "q",
            "samp",
            "kbd",
            "var",
            "b",
            "i",
            "u",
            "ruby",
            "rt",
            "rp",
            "bdi",
            "bdo",
            "span",
            "div",
            "root",
        ] {
            self.intern(tag);
        }

        for attr in &[
            "id",
            "class",
            "style",
            "src",
            "href",
            "alt",
            "title",
            "name",
            "value",
            "type",
            "placeholder",
            "disabled",
            "readonly",
            "required",
            "checked",
            "selected",
            "multiple",
            "size",
            "maxlength",
            "min",
            "max",
            "step",
            "pattern",
            "autofocus",
            "autocomplete",
            "novalidate",
            "formaction",
            "formmethod",
            "formtarget",
            "formenctype",
            "enctype",
            "method",
            "action",
            "target",
            "rel",
            "media",
            "charset",
            "content",
            "http-equiv",
            "property",
            "role",
            "aria-label",
            "aria-hidden",
            "aria-expanded",
            "aria-controls",
            "data-toggle",
            "data-target",
            "data-dismiss",
            "data-backdrop",
            "data-keyboard",
            "data-loading",
            "data-animation",
            "data-delay",
            "data-offset",
            "data-spy",
            "data-offset-top",
            "data-offset-bottom",
            "width",
            "height",
            "colspan",
            "rowspan",
            "scope",
            "headers",
            "abbr",
            "sorted",
            "reversed",
            "start",
            "download",
            "ping",
            "hreflang",
            "referrerpolicy",
            "loading",
            "decoding",
            "crossorigin",
            "integrity",
            "nonce",
            "async",
            "defer",
            "nomodule",
            "preload",
            "prefetch",
            "preconnect",
            "dns-picfetch",
        ] {
            self.intern(attr);
        }
    }
}

pub fn global_pool() -> std::sync::Arc<std::sync::Mutex<StringPool>> {
    use std::sync::{Arc, Mutex, OnceLock};

    static POOL: OnceLock<Arc<Mutex<StringPool>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let mut pool = StringPool::with_capacity(256);
        pool.preload_common();
        Arc::new(Mutex::new(pool))
    })
    .clone()
}

pub fn intern(s: &str) -> Arc<str> {
    global_pool()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .intern(s)
}

pub fn interned_savings_bytes() -> usize {
    global_pool()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .savings_bytes()
}

#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct InternedString(Arc<str>);

impl InternedString {
    pub fn new(value: &str) -> Self {
        Self(intern(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn as_arc(&self) -> &Arc<str> {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Default for InternedString {
    fn default() -> Self {
        Self(Arc::from(""))
    }
}

impl PartialEq<str> for InternedString {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<InternedString> for str {
    fn eq(&self, other: &InternedString) -> bool {
        *self == *other.0
    }
}

impl PartialEq<&str> for InternedString {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

impl PartialEq<InternedString> for &str {
    fn eq(&self, other: &InternedString) -> bool {
        *self == &*other.0
    }
}

impl PartialEq<String> for InternedString {
    fn eq(&self, other: &String) -> bool {
        *self.0 == **other
    }
}

impl PartialEq<InternedString> for String {
    fn eq(&self, other: &InternedString) -> bool {
        **self == *other.0
    }
}

impl PartialEq<Arc<str>> for InternedString {
    fn eq(&self, other: &Arc<str>) -> bool {
        self.0 == *other
    }
}

impl PartialEq<InternedString> for Arc<str> {
    fn eq(&self, other: &InternedString) -> bool {
        **self == *other.0
    }
}

impl std::ops::Deref for InternedString {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for InternedString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::borrow::Borrow<str> for InternedString {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for InternedString {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl From<&str> for InternedString {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for InternedString {
    fn from(value: String) -> Self {
        Self::new(&value)
    }
}

impl From<InternedString> for String {
    fn from(value: InternedString) -> Self {
        String::from(value.as_str())
    }
}

impl serde::Serialize for InternedString {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for InternedString {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        Ok(Self::new(&value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_basic() {
        let mut pool = StringPool::new();
        let s1 = pool.intern("div");
        let s2 = pool.intern("div");
        let s3 = pool.intern("span");

        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn test_pool_deduplication() {
        let mut pool = StringPool::new();
        let s1 = pool.intern("hello world");
        let s2 = pool.intern("hello world");
        let s3 = pool.intern("hello world");

        assert!(Arc::ptr_eq(&s1, &s2));
        assert!(Arc::ptr_eq(&s2, &s3));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_pool_intern_owned() {
        let mut pool = StringPool::new();
        let s1 = pool.intern("div");
        let s2 = pool.intern_owned("div".to_string());

        assert!(Arc::ptr_eq(&s1, &s2));
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn test_pool_contains() {
        let mut pool = StringPool::new();
        pool.intern("div");

        assert!(pool.contains("div"));
        assert!(!pool.contains("span"));
    }

    #[test]
    fn interning_is_bounded_and_reports_savings() {
        let mut pool = StringPool::new();
        let first = pool.intern("div");
        let second = pool.intern("div");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(pool.held_bytes(), 3);
        assert_eq!(pool.savings_bytes(), 3);

        for index in 0..resource_caps::INTERNED_STRINGS_MAX_ENTRIES + 64 {
            pool.intern(&format!("attribute-{index}"));
        }
        assert_eq!(pool.len(), resource_caps::INTERNED_STRINGS_MAX_ENTRIES);
        assert!(pool.held_bytes() <= resource_caps::INTERNED_STRINGS_MAX_BYTES);

        let beyond_cap = pool.intern("never-stored-value");
        assert_eq!(&*beyond_cap, "never-stored-value");
        assert_eq!(pool.len(), resource_caps::INTERNED_STRINGS_MAX_ENTRIES);

        pool.clear();
        assert_eq!(pool.held_bytes(), 0);
        assert_eq!(pool.savings_bytes(), 0);
    }

    #[test]
    fn interned_strings_compare_and_round_trip_like_str() {
        let value = InternedString::new("section");
        assert_eq!(value, "section");
        assert_eq!("section", value);
        assert_eq!(value.as_str(), "section");
        assert_eq!(value.len(), 7);
        assert!(value.starts_with("sec"));
        assert!(!value.is_empty());
        assert_eq!(format!("<{value}>"), "<section>");
        let owned: String = value.clone().into();
        assert_eq!(owned, "section");

        let mut counts: HashMap<InternedString, u8> = HashMap::new();
        counts.insert(value.clone(), 1);
        assert_eq!(counts.get("section"), Some(&1));

        let encoded = serde_json::to_string(&value).expect("serialise an interned name");
        assert_eq!(encoded, "\"section\"");
        let decoded: InternedString =
            serde_json::from_str(&encoded).expect("restore an interned name");
        assert_eq!(decoded, "section");
        assert!(Arc::ptr_eq(value.as_arc(), decoded.as_arc()));
    }

    #[test]
    fn test_pool_preload() {
        let mut pool = StringPool::new();
        pool.preload_common();

        assert!(pool.len() > 50);
        assert!(pool.contains("div"));
        assert!(pool.contains("class"));
        assert!(pool.contains("href"));
    }

    #[test]
    fn test_pool_clear() {
        let mut pool = StringPool::new();
        pool.intern("div");
        pool.intern("span");
        assert_eq!(pool.len(), 2);

        pool.clear();
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn test_pool_empty() {
        let pool = StringPool::new();
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn test_pool_capacity() {
        let pool = StringPool::with_capacity(100);
        assert!(pool.capacity() >= 100);
    }

    #[test]
    fn test_global_pool_interning() {
        let s1 = intern("div");
        let s2 = intern("div");
        let s3 = intern("span");

        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }
}
