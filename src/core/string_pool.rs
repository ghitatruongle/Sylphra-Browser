use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Default)]
pub struct StringPool {
    inner: HashMap<Arc<str>, Arc<str>>,
}

impl StringPool {
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: HashMap::with_capacity(capacity),
        }
    }

    pub fn intern(&mut self, s: &str) -> Arc<str> {
        if let Some(existing) = self.inner.get(s) {
            return existing.clone();
        }
        let arc: Arc<str> = Arc::from(s);
        self.inner.insert(arc.clone(), arc.clone());
        arc
    }

    pub fn intern_owned(&mut self, s: String) -> Arc<str> {
        if let Some(existing) = self.inner.get(s.as_str()) {
            return existing.clone();
        }
        let arc: Arc<str> = Arc::from(s);
        self.inner.insert(arc.clone(), arc.clone());
        arc
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
