use std::collections::HashMap;

pub const MAX_DOM_DEPTH: usize = 64;

pub const MAX_HTML_BYTES: usize = 10 * 1024 * 1024;

pub const MAX_ATTRS_PER_ELEMENT: usize = 128;
pub const MAX_ATTR_NAME_LEN: usize = 128;
pub const MAX_ATTR_VALUE_LEN: usize = 64 * 1024;

pub const MAX_RAW_TEXT_LEN: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Element {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_id: Option<u64>,
    pub tag: String,
    pub attrs: HashMap<String, String>,
    pub children: Vec<Element>,
    pub text: String,

    pub is_void: bool,
}

impl Element {
    pub fn new(tag: &str) -> Self {
        Element {
            node_id: None,
            tag: tag.to_string(),
            attrs: HashMap::new(),
            children: Vec::new(),
            text: String::new(),
            is_void: false,
        }
    }

    pub fn add_attr(&mut self, key: &str, value: &str) {
        self.attrs.insert(key.to_string(), value.to_string());
    }

    pub fn get_attr(&self, key: &str) -> Option<&String> {
        self.attrs.get(key)
    }

    pub fn add_child(&mut self, child: Element) {
        self.children.push(child);
    }

    pub fn find_tag(&self, tag: &str) -> Option<&Element> {
        let mut stack = vec![self];
        while let Some(node) = stack.pop() {
            if node.tag == tag {
                return Some(node);
            }
            for child in node.children.iter().rev() {
                stack.push(child);
            }
        }
        None
    }

    pub fn find_all_tags(&self, tag: &str) -> Vec<&Element> {
        let mut results = Vec::new();
        let mut stack = vec![self];
        while let Some(node) = stack.pop() {
            if node.tag == tag {
                results.push(node);
            }
            for child in node.children.iter().rev() {
                stack.push(child);
            }
        }
        results
    }

    pub fn text_content(&self) -> String {
        let mut result = String::new();
        let mut stack = vec![self];

        let mut ordered: Vec<&Element> = Vec::new();
        while let Some(node) = stack.pop() {
            ordered.push(node);
            for child in node.children.iter().rev() {
                stack.push(child);
            }
        }

        for node in ordered {
            if node.text.is_empty() {
                continue;
            }
            if !result.is_empty()
                && !result.ends_with(char::is_whitespace)
                && !node.text.starts_with(char::is_whitespace)
            {
                result.push(' ');
            }
            result.push_str(&node.text);
        }
        result
    }

    pub fn to_html(&self) -> String {
        let mut html = String::new();

        if self.tag == "root" {
            for child in &self.children {
                html.push_str(&child.to_html());
            }
            return html;
        }

        let mut attrs_str = String::new();
        for (k, v) in &self.attrs {
            attrs_str.push_str(&format!(" {}=\"{}\"", k, v));
        }

        if self.is_void {
            html.push_str(&format!("<{}{} />", self.tag, attrs_str));
        } else {
            html.push_str(&format!("<{}{}>", self.tag, attrs_str));
            if !self.text.is_empty() {
                if matches!(self.tag.as_str(), "script" | "style") {
                    html.push_str(&self.text);
                } else {
                    html.push_str(&escape_html(&self.text));
                }
            }
            for child in &self.children {
                html.push_str(&child.to_html());
            }
            html.push_str(&format!("</{}>", self.tag));
        }

        html
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn chars_match_ci_at(chars: &[char], pos: usize, s: &str) -> bool {
    let s: Vec<char> = s.chars().collect();
    if pos + s.len() > chars.len() {
        return false;
    }
    chars[pos..pos + s.len()]
        .iter()
        .zip(s.iter())
        .all(|(a, b)| a.eq_ignore_ascii_case(b))
}

fn decode_html_entities(text: &str) -> String {
    let mut result = String::new();
    let mut i = 0;
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();

    while i < len {
        if chars[i] == '&' {
            let scan_end = (i + 33).min(len);
            let entity_end = chars[i + 1..scan_end]
                .iter()
                .position(|&c| c == ';')
                .map(|j| i + 1 + j);

            if let Some(entity_end) = entity_end {
                if entity_end - i > 2 {
                    let entity: String = chars[i + 1..entity_end].iter().collect();
                    let decoded: String = match entity.as_str() {
                        "amp" => "&".to_string(),
                        "lt" => "<".to_string(),
                        "gt" => ">".to_string(),
                        "quot" => "\"".to_string(),
                        "apos" => "'".to_string(),
                        "nbsp" => " ".to_string(),
                        "copy" => "©".to_string(),
                        "reg" => "®".to_string(),
                        "trade" => "™".to_string(),
                        _ => {
                            if let Some(hex) = entity.strip_prefix("#x") {
                                if let Ok(codepoint) = u32::from_str_radix(hex, 16) {
                                    let codepoint = if codepoint == 0 { 0xFFFD } else { codepoint };
                                    if let Some(c) = char::from_u32(codepoint) {
                                        c.to_string()
                                    } else {
                                        format!("&{};", entity)
                                    }
                                } else {
                                    format!("&{};", entity)
                                }
                            } else if let Some(dec) = entity.strip_prefix('#') {
                                if let Ok(codepoint) = dec.parse::<u32>() {
                                    if let Some(c) = char::from_u32(codepoint) {
                                        c.to_string()
                                    } else {
                                        format!("&{};", entity)
                                    }
                                } else {
                                    format!("&{};", entity)
                                }
                            } else {
                                format!("&{};", entity)
                            }
                        }
                    };
                    result.push_str(&decoded);
                    i = entity_end + 1;
                    continue;
                }
            }

            result.push('&');
            i += 1;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

fn is_void_tag(tag: &str) -> bool {
    matches!(
        tag,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn is_raw_text_tag(tag: &str) -> bool {
    matches!(tag, "script" | "style")
}

fn is_optional_closing_tag(tag: &str) -> bool {
    matches!(
        tag,
        "p" | "li" | "dt" | "dd" | "option" | "thead" | "tbody" | "tfoot" | "tr" | "th" | "td"
    )
}

fn should_auto_close_parent(parent_tag: &str, child_tag: &str) -> bool {
    matches!(
        (parent_tag, child_tag),
        (
            "p",
            "p" | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "div"
                | "ul"
                | "ol"
                | "dl"
                | "table"
                | "form"
                | "section"
                | "article"
                | "header"
                | "footer"
                | "nav"
                | "main"
                | "aside"
        ) | (
            "a" | "span" | "em" | "strong" | "b" | "i" | "u" | "code",
            "p"
        ) | ("li", "li")
            | ("dt" | "dd", "dt" | "dd")
            | ("option", "option")
            | ("tr", "tr")
            | ("th" | "td", "th" | "td" | "tr")
            | ("thead" | "tbody" | "tfoot", "thead" | "tbody" | "tfoot")
    )
}

pub fn parse_html(html: &str) -> Element {
    let html = if html.len() > MAX_HTML_BYTES {
        &html[..MAX_HTML_BYTES]
    } else {
        html
    };
    let html = html.trim();
    if html.is_empty() {
        let mut body = Element::new("body");
        body.text = String::new();
        return body;
    }

    let mut stack: Vec<Element> = Vec::new();
    let root = Element::new("root");
    stack.push(root);

    let mut pos = 0;
    let chars: Vec<char> = html.chars().collect();
    let len = chars.len();

    while pos < len {
        if chars[pos] == '<' {
            if pos + 1 < len && chars[pos + 1] == '/' {
                pos += 2;
                let start = pos;
                while pos < len && chars[pos] != '>' {
                    pos += 1;
                }
                let close_tag: String = chars[start..pos].iter().collect();

                let close_tag = close_tag
                    .split_whitespace()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase();
                pos += 1;

                if stack.len() > 1 {
                    let mut found = false;
                    for depth in (1..stack.len()).rev() {
                        if stack[depth].tag == close_tag {
                            while stack.len() > depth + 1 {
                                let Some(completed) = stack.pop() else {
                                    break;
                                };
                                if let Some(parent) = stack.last_mut() {
                                    parent.add_child(completed);
                                }
                            }
                            if let Some(completed) = stack.pop() {
                                if let Some(parent) = stack.last_mut() {
                                    parent.add_child(completed);
                                }
                            }
                            found = true;
                            break;
                        }
                    }

                    if !found && is_optional_closing_tag(&close_tag) {}
                }
            } else if pos + 1 < len && (chars[pos + 1] == '!' || chars[pos + 1] == '?') {
                if chars_match_ci_at(&chars, pos, "<!--") {
                    pos += 4;
                    if pos < len && chars[pos] == '>' {
                        pos += 1;
                    } else {
                        while pos + 2 < len
                            && !(chars[pos] == '-'
                                && chars[pos + 1] == '-'
                                && chars[pos + 2] == '>')
                        {
                            pos += 1;
                        }
                        if pos + 2 < len {
                            pos += 3;
                        } else {
                            pos = len;
                        }
                    }
                } else if chars_match_ci_at(&chars, pos, "<!doctype") {
                    while pos < len && chars[pos] != '>' {
                        pos += 1;
                    }
                    if pos < len {
                        pos += 1;
                    }
                } else {
                    while pos < len && chars[pos] != '>' {
                        pos += 1;
                    }
                    if pos < len {
                        pos += 1;
                    }
                }
            } else if pos + 1 >= len || !chars[pos + 1].is_ascii_alphabetic() {
                if let Some(current) = stack.last_mut() {
                    if !current.text.is_empty() {
                        current.text.push(' ');
                    }
                    current.text.push('<');
                }
                pos += 1;
            } else {
                pos += 1;

                while pos < len && chars[pos].is_whitespace() {
                    pos += 1;
                }

                let start = pos;
                while pos < len
                    && chars[pos] != '>'
                    && chars[pos] != '/'
                    && !chars[pos].is_whitespace()
                    && chars[pos] != '<'
                {
                    pos += 1;
                }
                let tag_name: String = chars[start..pos].iter().collect();
                let tag_name = tag_name.trim().to_lowercase();

                if tag_name.is_empty() {
                    while pos < len && chars[pos] != '>' {
                        pos += 1;
                    }
                    if pos < len {
                        pos += 1;
                    }
                    continue;
                }

                let mut elem = Element::new(&tag_name);

                let is_raw = is_raw_text_tag(&tag_name);

                while pos < len && chars[pos] != '>' && chars[pos] != '/' {
                    while pos < len && chars[pos].is_whitespace() {
                        pos += 1;
                    }
                    if pos >= len || chars[pos] == '>' || chars[pos] == '/' {
                        break;
                    }

                    let k_start = pos;
                    while pos < len
                        && chars[pos] != '='
                        && chars[pos] != '>'
                        && chars[pos] != '/'
                        && chars[pos] != '<'
                        && !chars[pos].is_whitespace()
                    {
                        pos += 1;
                    }
                    if pos < len && chars[pos] == '<' {
                        break;
                    }
                    let mut key: String = chars[k_start..pos].iter().collect();
                    key = key.to_lowercase();

                    if key.len() > MAX_ATTR_NAME_LEN {
                        while pos < len && chars[pos] != '>' && chars[pos] != '/' {
                            if chars[pos] == '"' || chars[pos] == '\'' {
                                let q = chars[pos];
                                pos += 1;
                                while pos < len && chars[pos] != q {
                                    pos += 1;
                                }
                                if pos < len {
                                    pos += 1;
                                }
                            } else if chars[pos].is_whitespace() {
                                break;
                            } else {
                                pos += 1;
                            }
                        }
                        continue;
                    }

                    while pos < len && chars[pos].is_whitespace() {
                        pos += 1;
                    }

                    let mut val = String::new();
                    if pos < len && chars[pos] == '=' {
                        pos += 1;

                        while pos < len && chars[pos].is_whitespace() {
                            pos += 1;
                        }

                        if pos < len && (chars[pos] == '"' || chars[pos] == '\'') {
                            let quote = chars[pos];
                            pos += 1;
                            let v_start = pos;
                            let mut v_len = 0usize;
                            while pos < len && chars[pos] != quote && v_len < MAX_ATTR_VALUE_LEN {
                                pos += 1;
                                v_len += 1;
                            }
                            val = chars[v_start..pos].iter().collect();

                            while pos < len && chars[pos] != quote && chars[pos] != '>' {
                                pos += 1;
                            }
                            if pos < len && chars[pos] == quote {
                                pos += 1;
                            }
                        } else if pos < len && chars[pos] != '>' {
                            let v_start = pos;
                            let mut v_len = 0usize;
                            while pos < len
                                && !chars[pos].is_whitespace()
                                && chars[pos] != '>'
                                && v_len < MAX_ATTR_VALUE_LEN
                            {
                                pos += 1;
                                v_len += 1;
                            }
                            val = chars[v_start..pos].iter().collect();
                        }

                        val = decode_html_entities(&val);
                    }

                    if !key.is_empty() {
                        if elem.attrs.len() >= MAX_ATTRS_PER_ELEMENT {
                        } else {
                            elem.add_attr(&key, &val);
                        }
                    }
                }

                let mut is_self_closing = false;
                if pos < len && chars[pos] == '/' {
                    let mut j = pos + 1;
                    while j < len && chars[j].is_whitespace() {
                        j += 1;
                    }
                    if j < len && chars[j] == '>' {
                        is_self_closing = true;
                        pos = j;
                    }
                }
                if pos < len && chars[pos] == '<' {
                } else {
                    while pos < len && chars[pos] != '>' {
                        pos += 1;
                    }
                    if pos < len && chars[pos] == '>' {
                        pos += 1;
                    }
                }

                let is_void = is_void_tag(&tag_name);
                elem.is_void = is_void || is_self_closing;

                if !elem.is_void && stack.len() > 1 {
                    while stack.len() > 1
                        && stack
                            .last()
                            .is_some_and(|parent| should_auto_close_parent(&parent.tag, &tag_name))
                    {
                        let Some(completed) = stack.pop() else {
                            break;
                        };
                        if let Some(parent) = stack.last_mut() {
                            parent.add_child(completed);
                        }
                    }
                }

                if is_void || is_self_closing {
                    if let Some(parent) = stack.last_mut() {
                        parent.add_child(elem);
                    }
                } else if is_raw {
                    let close_tag: Vec<char> = format!("</{}>", tag_name).chars().collect();
                    let close_len = close_tag.len();

                    let mut end_pos = None;
                    let mut k = pos;
                    while k + close_len <= len {
                        if chars[k..k + close_len]
                            .iter()
                            .zip(close_tag.iter())
                            .all(|(a, b)| a.eq_ignore_ascii_case(b))
                        {
                            end_pos = Some(k);
                            break;
                        }
                        k += 1;
                    }

                    match end_pos {
                        Some(end) => {
                            let mut raw_text: String = chars[pos..end].iter().collect();
                            if raw_text.len() > MAX_RAW_TEXT_LEN {
                                raw_text.truncate(MAX_RAW_TEXT_LEN);
                            }
                            elem.text = raw_text;
                            pos = end + close_len;
                        }
                        None => {
                            let end = (pos + MAX_RAW_TEXT_LEN).min(len);
                            let raw_text: String = chars[pos..end].iter().collect();
                            elem.text = raw_text;
                            pos = len;
                        }
                    }

                    if let Some(parent) = stack.last_mut() {
                        parent.add_child(elem);
                    }
                } else if stack.len() + 1 > MAX_DOM_DEPTH {
                    if let Some(parent) = stack.last_mut() {
                        parent.add_child(elem);
                    }
                } else {
                    stack.push(elem);
                }
            }
        } else {
            let start = pos;
            while pos < len && chars[pos] != '<' {
                pos += 1;
            }
            let mut text: String = chars[start..pos].iter().collect();
            text = decode_html_entities(&text);
            let trimmed_text = text.trim();
            if !trimmed_text.is_empty() {
                if let Some(current) = stack.last_mut() {
                    if current.text.is_empty() {
                        current.text = trimmed_text.to_string();
                    } else {
                        current.text.push(' ');
                        current.text.push_str(trimmed_text);
                    }
                }
            }
        }
    }

    while stack.len() > 1 {
        let Some(completed) = stack.pop() else {
            break;
        };
        if let Some(parent) = stack.last_mut() {
            parent.add_child(completed);
        }
    }

    let mut root = stack.pop().unwrap_or_else(|| Element::new("html"));

    if root.children.len() == 1 && root.text.is_empty() {
        root.children.remove(0)
    } else {
        root.tag = "html".to_string();
        root
    }
}

pub fn fallback_parser(html: &str) -> Element {
    let mut elem = Element::new("div");
    let mut in_tag = false;
    let mut text = String::new();
    for c in html.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            text.push(c);
        }
    }
    elem.text = text.trim().to_string();
    elem
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_html() {
        let html = "<html><body><h1>Hello</h1></body></html>";
        let dom = parse_html(html);
        assert!(dom.find_tag("h1").is_some());
        assert_eq!(dom.find_tag("h1").unwrap().text, "Hello");
    }

    #[test]
    fn test_parse_nested_elements() {
        let html = "<div><ul><li>Item 1</li><li>Item 2</li></ul></div>";
        let dom = parse_html(html);
        let lis = dom.find_all_tags("li");
        assert_eq!(lis.len(), 2);
    }

    #[test]
    fn test_parse_with_attributes() {
        let html = r#"<a href="https://example.com" class="link">Click</a>"#;
        let dom = parse_html(html);
        let links = dom.find_all_tags("a");
        assert_eq!(links.len(), 1);
        assert_eq!(
            links[0].get_attr("href"),
            Some(&"https://example.com".to_string())
        );
        assert_eq!(links[0].get_attr("class"), Some(&"link".to_string()));
    }

    #[test]
    fn test_void_elements() {
        let html = "<div><img src='test.png'><br><hr></div>";
        let dom = parse_html(html);
        assert!(dom.find_tag("img").is_some());
        assert!(dom.find_tag("br").is_some());
        assert!(dom.find_tag("hr").is_some());
        assert!(dom.find_tag("img").unwrap().is_void);
    }

    #[test]
    fn test_self_closing_tag() {
        let html = "<div><img src='test.png' /></div>";
        let dom = parse_html(html);
        assert!(dom.find_tag("img").is_some());
    }

    #[test]
    fn test_html_entities() {
        let html = "<p>A &amp; B &lt; C &gt; D</p>";
        let dom = parse_html(html);
        let p = dom.find_tag("p").unwrap();
        assert_eq!(p.text, "A & B < C > D");
    }

    #[test]
    fn test_missing_closing_tag() {
        let html = "<div><p>Text without closing";
        let dom = parse_html(html);
        assert!(dom.find_tag("p").is_some());
        assert!(dom.find_tag("div").is_some());
    }

    #[test]
    fn test_script_tag_raw_text() {
        let html = "<script>var x = 1 < 2;</script><p>After</p>";
        let dom = parse_html(html);
        assert!(dom.find_tag("script").is_some());
        assert!(dom.find_tag("p").is_some());
        let script = dom.find_tag("script").unwrap();
        assert!(script.text.contains("var x"));
    }

    #[test]
    fn test_comment_ignored() {
        let html = "<div><!-- comment --><p>Text</p></div>";
        let dom = parse_html(html);
        let p = dom.find_tag("p");
        assert!(p.is_some());
        assert_eq!(p.unwrap().text, "Text");
    }

    #[test]
    fn test_decode_numeric_entities() {
        assert_eq!(decode_html_entities("&#65;"), "A");
        assert_eq!(decode_html_entities("&#x41;"), "A");
    }

    #[test]
    fn test_boolean_attribute() {
        let html = "<input disabled required>";
        let dom = parse_html(html);
        let input = dom.find_tag("input").unwrap();
        assert_eq!(input.get_attr("disabled"), Some(&"".to_string()));
        assert_eq!(input.get_attr("required"), Some(&"".to_string()));
    }

    #[test]
    fn test_invalid_markup_recovery() {
        let html = "<p>Hello << world</p>";
        let dom = parse_html(html);
        assert!(dom.find_tag("p").is_some());
    }

    #[test]
    fn test_text_content_recursive() {
        let html = "<div><p>Hello <b>World</b></p></div>";
        let dom = parse_html(html);
        assert!(dom.find_tag("p").is_some());
        assert_eq!(dom.find_tag("b").unwrap().text, "World");
    }

    #[test]
    fn test_unterminated_entity_keeps_text() {
        assert_eq!(decode_html_entities("Fish & Chips"), "Fish & Chips");
        assert_eq!(decode_html_entities("R&D"), "R&D");
        assert_eq!(decode_html_entities("A &amp; B & raw"), "A & B & raw");
    }

    #[test]
    fn test_multibyte_utf8_no_panic() {
        let dom = parse_html("é<!-- comment --><p>ok</p>");
        assert_eq!(dom.find_tag("p").unwrap().text, "ok");

        let dom = parse_html("héllo<!DOCTYPE html><p>x</p>");
        assert_eq!(dom.find_tag("p").unwrap().text, "x");
    }

    #[test]
    fn test_uppercase_close_script() {
        let html = "<SCRIPT>var x = 1 < 2</SCRIPT><p>After</p>";
        let dom = parse_html(html);
        assert!(dom.find_tag("script").is_some());
        assert!(dom.find_tag("script").unwrap().text.contains("var x"));
        assert_eq!(dom.find_tag("p").unwrap().text, "After");
    }

    #[test]
    fn test_style_tag_raw_text() {
        let html = "<style>p { color: red; }</style><p>x</p>";
        let dom = parse_html(html);
        assert!(dom.find_tag("style").unwrap().text.contains("color: red"));
        assert_eq!(dom.find_tag("p").unwrap().text, "x");
    }

    #[test]
    fn test_textarea_is_not_raw() {
        let html = "<textarea>a &amp; b</textarea>";
        let dom = parse_html(html);
        assert_eq!(dom.find_tag("textarea").unwrap().text, "a & b");
    }

    #[test]
    fn test_parse_depth_is_capped() {
        let html = format!("<html><body>{}</body></html>", "<div>".repeat(100_000));
        let dom = parse_html(&html);

        let mut stack: Vec<(usize, &Element)> = vec![(0, &dom)];
        let mut max_depth = 0usize;
        let mut visited = 0usize;
        while let Some((d, el)) = stack.pop() {
            visited += 1;
            max_depth = max_depth.max(d);
            for c in &el.children {
                stack.push((d + 1, c));
            }
        }
        assert!(visited > 10_000, "parser should still build many nodes");
        assert!(
            max_depth <= MAX_DOM_DEPTH,
            "DOM depth {} exceeds cap {}",
            max_depth,
            MAX_DOM_DEPTH
        );
    }

    #[test]
    fn test_unquoted_attribute_with_slash() {
        let html = "<a href=/foo>text</a>";
        let dom = parse_html(html);
        let a = dom.find_tag("a").unwrap();
        assert_eq!(a.get_attr("href").map(|s| s.as_str()), Some("/foo"));
        assert!(!a.is_void, "<a href=/foo> must not be self-closing");
        assert_eq!(a.text, "text", "text after the tag must not be swallowed");
    }

    #[test]
    fn test_unquoted_attr_with_trailing_slash_not_self_closing() {
        let html = "<a href=/foo/>bar</a>";
        let dom = parse_html(html);
        let a = dom.find_tag("a").unwrap();
        assert_eq!(a.get_attr("href").map(|s| s.as_str()), Some("/foo/"));
        assert!(!a.is_void);
        assert_eq!(a.text, "bar");
    }

    #[test]
    fn test_self_closing_slash_must_precede_gt() {
        let html = "<div class=\"x\" />tail";
        let dom = parse_html(html);
        let div = dom.find_tag("div").unwrap();
        assert!(div.is_void);
    }

    #[test]
    fn test_p_auto_close_closes_inline_chain() {
        let html = "<p>one<a>two<p>three</p>";
        let dom = parse_html(html);
        let ps = dom.find_all_tags("p");
        assert_eq!(ps.len(), 2, "expected two sibling <p> elements");
        let p1 = &ps[0];
        assert_eq!(p1.text, "one");

        assert!(p1.find_tag("a").is_some());

        let p1_parent_tag = dom
            .find_tag("p")
            .and_then(|_| ps[0].find_tag("a"))
            .map(|a| a.text.as_str());
        assert_eq!(p1_parent_tag, Some("two"));
        let p2 = &ps[1];
        assert_eq!(p2.text, "three");
    }
}
