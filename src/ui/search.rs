use std::time::Duration;

use crate::parser::parse_html;

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

const DDG_HTML_URL: &str = "https://html.duckduckgo.com/html/?q=";

const SEARCH_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                         (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

pub fn search_web(query: &str) -> Result<Vec<SearchResult>, String> {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let url = format!("{}{}", DDG_HTML_URL, encoded);

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(15))
        .redirects(5)
        .user_agent(SEARCH_UA)
        .build();

    let response = agent.get(&url).call().map_err(|e| e.to_string())?;
    let status = response.status();
    let body = response.into_string().map_err(|e| e.to_string())?;

    if !(200..300).contains(&status) {
        return Err(format!("Search engine returned status {}", status));
    }

    Ok(parse_ddg_html(&body))
}

pub async fn search_web_async(query: &str) -> Result<Vec<SearchResult>, String> {
    search_web_async_with_cancellation(
        query,
        crate::network_scheduler::CancellationToken::default(),
    )
    .await
}

pub async fn search_web_async_with_cancellation(
    query: &str,
    cancellation: crate::network_scheduler::CancellationToken,
) -> Result<Vec<SearchResult>, String> {
    let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
    let response = crate::network_scheduler::fetch_shared(
        format!("{}{}", DDG_HTML_URL, encoded),
        String::new(),
        1,
        crate::network_scheduler::RequestPriority::Navigation,
        crate::network_scheduler::ResponseMode::Document,
        cancellation,
    )
    .await?;
    if !(200..300).contains(&response.status_code) {
        return Err(format!(
            "Search engine returned status {}",
            response.status_code
        ));
    }
    Ok(parse_ddg_html(&response.body))
}

pub fn parse_ddg_html(html: &str) -> Vec<SearchResult> {
    let dom = parse_html(html);
    let mut results = Vec::new();

    for div in dom.find_all_tags("div") {
        if !has_class(div, "result") {
            continue;
        }
        let snippet = find_descendant(div, "a", "result__snippet")
            .map(|a| clean_whitespace(&a.text_content()))
            .unwrap_or_default();

        if let Some(title_a) = find_descendant(div, "a", "result__a") {
            let title = clean_whitespace(&title_a.text_content());
            let url = title_a
                .get_attr("href")
                .and_then(|href| decode_ddg_url(href))
                .unwrap_or_default();
            if !title.is_empty() && !url.is_empty() {
                results.push(SearchResult {
                    title,
                    url,
                    snippet,
                });
            }
        }
    }

    results
}

fn has_class(el: &crate::parser::Element, needle: &str) -> bool {
    el.get_attr("class")
        .map(|c| c.split_whitespace().any(|tok| tok == needle))
        .unwrap_or(false)
}

fn find_descendant<'a>(
    el: &'a crate::parser::Element,
    tag: &str,
    class: &str,
) -> Option<&'a crate::parser::Element> {
    if el.tag == tag && has_class(el, class) {
        return Some(el);
    }
    el.children
        .iter()
        .find_map(|child| find_descendant(child, tag, class))
}

fn decode_ddg_url(href: &str) -> Option<String> {
    let raw = href.split("uddg=").nth(1)?.split('&').next()?;
    if raw.is_empty() {
        return None;
    }
    let decoded = url::form_urlencoded::parse(format!("q={}", raw).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())?;
    if decoded.starts_with("http://") || decoded.starts_with("https://") {
        Some(decoded)
    } else {
        None
    }
}

fn clean_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ddg_html_basic() {
        let html = r#"
            <html><body>
            <div class="result">
                <h2 class="result__title">
                    <a rel="nofollow" class="result__a"
                       href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=abc">The
                       Rust Programming Language</a>
                </h2>
                <a class="result__snippet" href="//duckduckgo.com/l/?uddg=...">A language
                   empowering everyone to build reliable software.</a>
            </div>
            <div class="result">
                <h2 class="result__title">
                    <a rel="nofollow" class="result__a"
                       href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust-lang.org%2Fbook%2F&amp;rut=def">The
                       Rust Book</a>
                </h2>
                <a class="result__snippet" href="//duckduckgo.com/l/?uddg=...">Learn Rust with
                   the official book.</a>
            </div>
            </body></html>
        "#;

        let results = parse_ddg_html(html);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "The Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(
            results[0].snippet,
            "A language empowering everyone to build reliable software."
        );
        assert_eq!(results[1].url, "https://doc.rust-lang.org/book/");
    }

    #[test]
    fn test_parse_ddg_html_no_results() {
        let html = "<html><body><div class=\"no-results\">No results found</div></body></html>";
        assert!(parse_ddg_html(html).is_empty());
    }

    #[test]
    fn test_snippet_stays_aligned_when_title_filtered() {
        let html = r#"
            <html><body>
            <div class="result">
                <h2 class="result__title">
                    <a class="result__a" href="https://ads.example.com/landing">Ad without redirect</a>
                </h2>
                <a class="result__snippet">This snippet belongs to the filtered ad.</a>
            </div>
            <div class="result">
                <h2 class="result__title">
                    <a class="result__a"
                       href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=abc">The
                       Rust Programming Language</a>
                </h2>
                <a class="result__snippet">A language empowering everyone.</a>
            </div>
            </body></html>
        "#;
        let results = parse_ddg_html(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].snippet, "A language empowering everyone.");
    }

    #[test]
    fn test_class_matches_exact_tokens_not_substrings() {
        let html = r#"
            <html><body>
            <div class="result__footer">
                <a class="not_a_result__a"
                   href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F&amp;rut=zzz">Decoy</a>
                <a class="result__snippet">Decoy snippet</a>
            </div>
            <div class="result">
                <a class="result__a"
                   href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust-lang.org%2F&amp;rut=abc">The
                   Rust Programming Language</a>
                <a class="result__snippet">A language empowering everyone.</a>
            </div>
            </body></html>
        "#;
        let results = parse_ddg_html(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "The Rust Programming Language");
        assert_eq!(results[0].url, "https://www.rust-lang.org/");
        assert_eq!(results[0].snippet, "A language empowering everyone.");
    }

    #[test]
    fn test_decode_ddg_url() {
        assert_eq!(
            decode_ddg_url(
                "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1%26c%3D2&rut=xyz"
            ),
            Some("https://example.com/a?b=1&c=2".to_string())
        );
        assert_eq!(decode_ddg_url("//example.com/direct"), None);
        assert_eq!(decode_ddg_url("//duckduckgo.com/l/?uddg="), None);
    }

    #[test]
    fn test_clean_whitespace() {
        assert_eq!(clean_whitespace("  a\n  b\t c  "), "a b c");
    }
}
