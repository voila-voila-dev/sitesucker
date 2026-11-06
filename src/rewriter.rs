use std::collections::HashMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use url::Url;

use crate::downloader::url_to_filepath_guess;

/// Compute a relative path from `from_file` to `to_file`.
fn relative_path(from_file: &Path, to_file: &Path) -> String {
    let from_dir = from_file.parent().unwrap_or(Path::new(""));
    let to_dir = to_file.parent().unwrap_or(Path::new(""));
    let to_filename = to_file.file_name().unwrap_or_default().to_string_lossy();

    let from_components: Vec<_> = from_dir.components().collect();
    let to_components: Vec<_> = to_dir.components().collect();

    let mut common = 0;
    for (a, b) in from_components.iter().zip(to_components.iter()) {
        if a == b {
            common += 1;
        } else {
            break;
        }
    }

    let ups = from_components.len() - common;
    let mut result = String::new();
    for _ in 0..ups {
        result.push_str("../");
    }
    for component in &to_components[common..] {
        result.push_str(&component.as_os_str().to_string_lossy());
        result.push('/');
    }
    result.push_str(&to_filename);

    if result.is_empty() {
        to_filename.to_string()
    } else {
        result
    }
}

/// Rewrite absolute URLs in HTML content to relative paths.
pub fn rewrite_html(
    html: &str,
    page_url: &Url,
    base_domain: &str,
    output_dir: &Path,
    include_subdomains: bool,
    filepath_map: &HashMap<String, PathBuf>,
    page_filepath: &Path,
) -> String {
    let mut result = html.to_string();

    // Attributes that contain a single URL
    // Includes standard attributes + common lazy-loading data attributes
    let url_attrs = r"(?:href|src|action|poster|data|data-src|data-lazy-src|data-original|data-bg|data-background-image)";

    // Double-quoted attributes
    let attr_dq = Regex::new(&format!(r#"({}\s*=\s*)"([^"]*?)""#, url_attrs)).unwrap();
    result = attr_dq
        .replace_all(&result, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let raw_url = &caps[2];
            if let Some(rewritten) = rewrite_url(
                raw_url,
                page_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                page_filepath,
            ) {
                format!("{}\"{}\"", prefix, rewritten)
            } else {
                caps[0].to_string()
            }
        })
        .to_string();

    // Single-quoted attributes
    let attr_sq = Regex::new(&format!(r"({}\s*=\s*)'([^']*?)'", url_attrs)).unwrap();
    result = attr_sq
        .replace_all(&result, |caps: &regex::Captures| {
            let prefix = &caps[1];
            let raw_url = &caps[2];
            if let Some(rewritten) = rewrite_url(
                raw_url,
                page_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                page_filepath,
            ) {
                format!("{}'{}'", prefix, rewritten)
            } else {
                caps[0].to_string()
            }
        })
        .to_string();

    // Srcset-like attributes (standard + lazy variants), double-quoted
    let srcset_attrs = r"(?:srcset|data-srcset|data-lazy-srcset)";
    let srcset_dq = Regex::new(&format!(r#"({}\s*=\s*)"([^"]*?)""#, srcset_attrs)).unwrap();
    result = srcset_dq
        .replace_all(&result, |caps: &regex::Captures| {
            rewrite_srcset(
                &caps[1],
                &caps[2],
                '"',
                page_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                page_filepath,
            )
        })
        .to_string();

    // Srcset-like attributes, single-quoted
    let srcset_sq = Regex::new(&format!(r"({}\s*=\s*)'([^']*?)'", srcset_attrs)).unwrap();
    result = srcset_sq
        .replace_all(&result, |caps: &regex::Captures| {
            rewrite_srcset(
                &caps[1],
                &caps[2],
                '\'',
                page_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                page_filepath,
            )
        })
        .to_string();

    // Rewrite url() inside inline style="" attributes (double-quoted)
    let style_dq = Regex::new(r#"style\s*=\s*"([^"]*?)""#).unwrap();
    result = style_dq
        .replace_all(&result, |caps: &regex::Captures| {
            let rewritten_style = rewrite_css_urls_inline(
                &caps[1],
                page_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                page_filepath,
            );
            format!("style=\"{}\"", rewritten_style)
        })
        .to_string();

    // Rewrite url() inside inline style='' attributes (single-quoted)
    let style_sq = Regex::new(r"style\s*=\s*'([^']*?)'").unwrap();
    result = style_sq
        .replace_all(&result, |caps: &regex::Captures| {
            let rewritten_style = rewrite_css_urls_inline(
                &caps[1],
                page_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                page_filepath,
            );
            format!("style='{}'", rewritten_style)
        })
        .to_string();

    // Rewrite url() inside <style> blocks
    let style_block = Regex::new(r"(?s)(<style[^>]*>)(.*?)(</style>)").unwrap();
    result = style_block
        .replace_all(&result, |caps: &regex::Captures| {
            let open_tag = &caps[1];
            let css_content = &caps[2];
            let close_tag = &caps[3];
            let rewritten = rewrite_css_urls_inline(
                css_content,
                page_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                page_filepath,
            );
            format!("{}{}{}", open_tag, rewritten, close_tag)
        })
        .to_string();

    result
}

/// Rewrite url() references within a CSS snippet (inline style or style block).
fn rewrite_css_urls_inline(
    css: &str,
    page_url: &Url,
    base_domain: &str,
    output_dir: &Path,
    include_subdomains: bool,
    filepath_map: &HashMap<String, PathBuf>,
    page_filepath: &Path,
) -> String {
    let url_re = Regex::new(r#"url\(\s*['"]?([^)'"]+?)['"]?\s*\)"#).unwrap();
    url_re
        .replace_all(css, |caps: &regex::Captures| {
            let raw_url = &caps[1];
            if let Some(rewritten) = rewrite_url(
                raw_url,
                page_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                page_filepath,
            ) {
                format!("url(\"{}\")", rewritten)
            } else {
                caps[0].to_string()
            }
        })
        .to_string()
}

fn rewrite_srcset(
    prefix: &str,
    srcset_val: &str,
    quote: char,
    page_url: &Url,
    base_domain: &str,
    output_dir: &Path,
    include_subdomains: bool,
    filepath_map: &HashMap<String, PathBuf>,
    page_filepath: &Path,
) -> String {
    let rewritten_entries: Vec<String> = srcset_val
        .split(',')
        .map(|entry| {
            let parts: Vec<&str> = entry.trim().splitn(2, char::is_whitespace).collect();
            if let Some(rewritten) = rewrite_url(
                parts[0],
                page_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                page_filepath,
            ) {
                if parts.len() > 1 {
                    format!("{} {}", rewritten, parts[1])
                } else {
                    rewritten
                }
            } else {
                entry.trim().to_string()
            }
        })
        .collect();
    format!(
        "{}{}{}{}",
        prefix,
        quote,
        rewritten_entries.join(", "),
        quote
    )
}

/// Rewrite URLs in CSS content to relative paths, including @import.
pub fn rewrite_css(
    css: &str,
    css_url: &Url,
    base_domain: &str,
    output_dir: &Path,
    include_subdomains: bool,
    filepath_map: &HashMap<String, PathBuf>,
    css_filepath: &Path,
) -> String {
    let mut result = css.to_string();

    // Rewrite url(...)
    let url_re = Regex::new(r#"url\(\s*['"]?([^)'"]+?)['"]?\s*\)"#).unwrap();
    result = url_re
        .replace_all(&result, |caps: &regex::Captures| {
            let raw_url = &caps[1];
            if let Some(rewritten) = rewrite_url(
                raw_url,
                css_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                css_filepath,
            ) {
                format!("url({})", rewritten)
            } else {
                caps[0].to_string()
            }
        })
        .to_string();

    // Rewrite @import "..." (non-url form, double quotes)
    let import_dq = Regex::new(r#"@import\s+"([^"]+)""#).unwrap();
    result = import_dq
        .replace_all(&result, |caps: &regex::Captures| {
            let raw_url = &caps[1];
            if let Some(rewritten) = rewrite_url(
                raw_url,
                css_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                css_filepath,
            ) {
                format!("@import \"{}\"", rewritten)
            } else {
                caps[0].to_string()
            }
        })
        .to_string();

    // Rewrite @import '...' (single quotes)
    let import_sq = Regex::new(r"@import\s+'([^']+)'").unwrap();
    result = import_sq
        .replace_all(&result, |caps: &regex::Captures| {
            let raw_url = &caps[1];
            if let Some(rewritten) = rewrite_url(
                raw_url,
                css_url,
                base_domain,
                output_dir,
                include_subdomains,
                filepath_map,
                css_filepath,
            ) {
                format!("@import '{}'", rewritten)
            } else {
                caps[0].to_string()
            }
        })
        .to_string();

    result
}

/// Try to rewrite a single URL to a relative path.
fn rewrite_url(
    raw_url: &str,
    page_url: &Url,
    base_domain: &str,
    output_dir: &Path,
    include_subdomains: bool,
    filepath_map: &HashMap<String, PathBuf>,
    from_filepath: &Path,
) -> Option<String> {
    let trimmed = raw_url.trim();

    if trimmed.starts_with("data:")
        || trimmed.starts_with('#')
        || trimmed.starts_with("mailto:")
        || trimmed.starts_with("tel:")
        || trimmed.starts_with("javascript:")
    {
        return None;
    }

    let resolved = page_url.join(trimmed).ok()?;

    if !is_same_site(&resolved, base_domain, include_subdomains) {
        return None;
    }

    // Strip fragment for lookup
    let mut lookup_url = resolved.clone();
    lookup_url.set_fragment(None);
    let lookup_key = lookup_url.as_str().to_string();

    // Use the actual filepath if we have it, otherwise guess
    let to_path = filepath_map
        .get(&lookup_key)
        .cloned()
        .unwrap_or_else(|| url_to_filepath_guess(&lookup_url, output_dir));

    let rel = relative_path(from_filepath, &to_path);

    let fragment = resolved
        .fragment()
        .map(|f| format!("#{}", f))
        .unwrap_or_default();

    Some(format!("{}{}", rel, fragment))
}

fn is_same_site(url: &Url, base_domain: &str, include_subdomains: bool) -> bool {
    if let Some(host) = url.host_str() {
        if include_subdomains {
            host == base_domain || host.ends_with(&format!(".{}", base_domain))
        } else {
            host == base_domain
        }
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn empty_map() -> HashMap<String, PathBuf> {
        HashMap::new()
    }

    // --- relative_path ---

    #[test]
    fn relative_path_same_dir() {
        let from = Path::new("out/index.html");
        let to = Path::new("out/style.css");
        assert_eq!(relative_path(from, to), "style.css");
    }

    #[test]
    fn relative_path_child_dir() {
        let from = Path::new("out/index.html");
        let to = Path::new("out/css/style.css");
        assert_eq!(relative_path(from, to), "css/style.css");
    }

    #[test]
    fn relative_path_parent_dir() {
        let from = Path::new("out/blog/post/index.html");
        let to = Path::new("out/css/style.css");
        assert_eq!(relative_path(from, to), "../../css/style.css");
    }

    #[test]
    fn relative_path_deeply_nested() {
        let from = Path::new("out/a/b/c/index.html");
        let to = Path::new("out/x/y/file.js");
        assert_eq!(relative_path(from, to), "../../../x/y/file.js");
    }

    #[test]
    fn relative_path_same_file() {
        let from = Path::new("out/page/index.html");
        let to = Path::new("out/page/index.html");
        assert_eq!(relative_path(from, to), "index.html");
    }

    // --- is_same_site ---

    #[test]
    fn same_site_exact_match() {
        assert!(is_same_site(
            &u("https://example.com/page"),
            "example.com",
            false
        ));
    }

    #[test]
    fn same_site_different_domain() {
        assert!(!is_same_site(
            &u("https://other.com/page"),
            "example.com",
            false
        ));
    }

    #[test]
    fn same_site_subdomain_without_flag() {
        assert!(!is_same_site(
            &u("https://blog.example.com/page"),
            "example.com",
            false
        ));
    }

    #[test]
    fn same_site_subdomain_with_flag() {
        assert!(is_same_site(
            &u("https://blog.example.com/page"),
            "example.com",
            true
        ));
    }

    #[test]
    fn same_site_subdomain_not_suffix() {
        // "notexample.com" should NOT match "example.com" even with subdomains
        assert!(!is_same_site(
            &u("https://notexample.com/page"),
            "example.com",
            true
        ));
    }

    // --- rewrite_html ---

    #[test]
    fn rewrite_html_internal_link() {
        let html = r#"<a href="https://example.com/about">About</a>"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("about/index.html"));
        assert!(!result.contains("https://example.com"));
    }

    #[test]
    fn rewrite_html_external_link_unchanged() {
        let html = r#"<a href="https://other.com/page">Other</a>"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("https://other.com/page"));
    }

    #[test]
    fn rewrite_html_relative_link() {
        let html = r#"<a href="/blog/post">Post</a>"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("blog/post/index.html"));
    }

    #[test]
    fn rewrite_html_preserves_fragment() {
        let html = r#"<a href="https://example.com/page#section">Link</a>"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("#section"));
    }

    #[test]
    fn rewrite_html_skips_data_uri() {
        let html = r#"<img src="data:image/png;base64,abc123">"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("data:image/png;base64,abc123"));
    }

    #[test]
    fn rewrite_html_skips_mailto() {
        let html = r#"<a href="mailto:test@example.com">Email</a>"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("mailto:test@example.com"));
    }

    #[test]
    fn rewrite_html_skips_javascript() {
        let html = r#"<a href="javascript:void(0)">Click</a>"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("javascript:void(0)"));
    }

    #[test]
    fn rewrite_html_single_quotes() {
        let html = r#"<a href='https://example.com/about'>About</a>"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("about/index.html"));
        assert!(!result.contains("https://example.com"));
    }

    #[test]
    fn rewrite_html_img_src() {
        let html = r#"<img src="https://example.com/img/logo.png">"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("img/logo.png"));
    }

    #[test]
    fn rewrite_html_script_src() {
        let html = r#"<script src="/js/app.js"></script>"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("js/app.js"));
    }

    #[test]
    fn rewrite_html_from_nested_page() {
        let html = r#"<link href="/css/style.css" rel="stylesheet">"#;
        let page_url = u("https://example.com/blog/post/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/blog/post/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("../../css/style.css"));
    }

    #[test]
    fn rewrite_html_uses_filepath_map() {
        let html = r#"<img src="https://example.com/api/image/123">"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let mut map = HashMap::new();
        map.insert(
            "https://example.com/api/image/123".to_string(),
            PathBuf::from("out/api/image/123.png"),
        );

        let result = rewrite_html(html, &page_url, "example.com", out, false, &map, &page_fp);
        assert!(result.contains("api/image/123.png"));
    }

    #[test]
    fn rewrite_html_srcset() {
        let html = r#"<img srcset="https://example.com/img/a.jpg 1x, https://example.com/img/b.jpg 2x">"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("img/a.jpg 1x"));
        assert!(result.contains("img/b.jpg 2x"));
        assert!(!result.contains("https://example.com"));
    }

    // --- Lazy loading attribute rewriting ---

    #[test]
    fn rewrite_html_data_src() {
        let html = r#"<img data-src="https://example.com/img/lazy.jpg" src="placeholder.gif">"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains(r#"data-src="img/lazy.jpg""#));
    }

    #[test]
    fn rewrite_html_data_lazy_src() {
        let html = r#"<img data-lazy-src="https://example.com/wp-content/uploads/photo.png" src="https://example.com/wp-content/uploads/photo.png">"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains(r#"data-lazy-src="wp-content/uploads/photo.png""#));
        assert!(result.contains(r#"src="wp-content/uploads/photo.png""#));
    }

    #[test]
    fn rewrite_html_data_srcset() {
        let html = r#"<img data-srcset="https://example.com/img/a.jpg 300w, https://example.com/img/b.jpg 600w">"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("img/a.jpg 300w"));
        assert!(result.contains("img/b.jpg 600w"));
        assert!(!result.contains("https://example.com"));
    }

    #[test]
    fn rewrite_html_data_bg() {
        let html = r#"<div data-bg="https://example.com/img/hero.jpg"></div>"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains(r#"data-bg="img/hero.jpg""#));
    }

    #[test]
    fn rewrite_html_data_original() {
        let html = r#"<img data-original="/img/photo.jpg">"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("img/photo.jpg"));
        assert!(!result.contains("/img/photo.jpg"));
    }

    #[test]
    fn rewrite_html_data_background_image() {
        let html = r#"<section data-background-image="https://example.com/img/section.jpg"></section>"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains(r#"data-background-image="img/section.jpg""#));
    }

    #[test]
    fn rewrite_html_wordpress_lazy_pattern() {
        // Full WordPress lazy-loading pattern
        let html = r#"<img width="1153" height="1153" decoding="async" src="https://example.com/wp-content/uploads/2023/01/certif.png" class="lqd-image-icon" data-lazy-src="https://example.com/wp-content/uploads/2023/01/certif.png" data-ll-status="loaded">"#;
        let page_url = u("https://example.com/");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        // Both src and data-lazy-src should be rewritten
        assert!(!result.contains("https://example.com"), "Should not contain absolute URLs: {}", result);
        assert!(result.contains("wp-content/uploads/2023/01/certif.png"));
    }

    // --- rewrite_css ---

    #[test]
    fn rewrite_css_url() {
        let css = r#"body { background: url("/img/bg.png"); }"#;
        let css_url = u("https://example.com/css/style.css");
        let out = Path::new("out");
        let css_fp = PathBuf::from("out/css/style.css");

        let result = rewrite_css(css, &css_url, "example.com", out, false, &empty_map(), &css_fp);
        assert!(result.contains("../img/bg.png"));
    }

    #[test]
    fn rewrite_css_url_with_quotes() {
        let css = r#"body { background: url('/img/bg.png'); }"#;
        let css_url = u("https://example.com/css/style.css");
        let out = Path::new("out");
        let css_fp = PathBuf::from("out/css/style.css");

        let result = rewrite_css(css, &css_url, "example.com", out, false, &empty_map(), &css_fp);
        assert!(result.contains("../img/bg.png"));
    }

    #[test]
    fn rewrite_css_import_double_quotes() {
        let css = r#"@import "https://example.com/css/reset.css";"#;
        let css_url = u("https://example.com/css/style.css");
        let out = Path::new("out");
        let css_fp = PathBuf::from("out/css/style.css");

        let result = rewrite_css(css, &css_url, "example.com", out, false, &empty_map(), &css_fp);
        assert!(result.contains("@import \"reset.css\""));
    }

    #[test]
    fn rewrite_css_import_single_quotes() {
        let css = r#"@import 'https://example.com/css/reset.css';"#;
        let css_url = u("https://example.com/css/style.css");
        let out = Path::new("out");
        let css_fp = PathBuf::from("out/css/style.css");

        let result = rewrite_css(css, &css_url, "example.com", out, false, &empty_map(), &css_fp);
        assert!(result.contains("@import 'reset.css'"));
    }

    #[test]
    fn rewrite_css_import_relative() {
        let css = r#"@import "../fonts/font.css";"#;
        let css_url = u("https://example.com/css/style.css");
        let out = Path::new("out");
        let css_fp = PathBuf::from("out/css/style.css");

        let result = rewrite_css(css, &css_url, "example.com", out, false, &empty_map(), &css_fp);
        assert!(result.contains("../fonts/font.css"));
    }

    #[test]
    fn rewrite_css_external_url_unchanged() {
        let css = r#"body { background: url("https://cdn.other.com/img.png"); }"#;
        let css_url = u("https://example.com/css/style.css");
        let out = Path::new("out");
        let css_fp = PathBuf::from("out/css/style.css");

        let result = rewrite_css(css, &css_url, "example.com", out, false, &empty_map(), &css_fp);
        assert!(result.contains("https://cdn.other.com/img.png"));
    }

    #[test]
    fn rewrite_css_data_uri_unchanged() {
        let css = r#"body { background: url("data:image/svg+xml,<svg></svg>"); }"#;
        let css_url = u("https://example.com/css/style.css");
        let out = Path::new("out");
        let css_fp = PathBuf::from("out/css/style.css");

        let result = rewrite_css(css, &css_url, "example.com", out, false, &empty_map(), &css_fp);
        assert!(result.contains("data:image/svg+xml"));
    }

    #[test]
    fn rewrite_css_font_url() {
        let css = r#"@font-face { src: url("/fonts/roboto.woff2"); }"#;
        let css_url = u("https://example.com/css/style.css");
        let out = Path::new("out");
        let css_fp = PathBuf::from("out/css/style.css");

        let result = rewrite_css(css, &css_url, "example.com", out, false, &empty_map(), &css_fp);
        assert!(result.contains("../fonts/roboto.woff2"));
    }

    #[test]
    fn rewrite_css_multiple_urls() {
        let css = r#"
            body { background: url("/img/bg.png"); }
            .icon { background: url("/img/icon.svg"); }
        "#;
        let css_url = u("https://example.com/css/style.css");
        let out = Path::new("out");
        let css_fp = PathBuf::from("out/css/style.css");

        let result = rewrite_css(css, &css_url, "example.com", out, false, &empty_map(), &css_fp);
        assert!(result.contains("../img/bg.png"));
        assert!(result.contains("../img/icon.svg"));
    }

    #[test]
    fn rewrite_html_inline_style_background_image() {
        let html = r#"<div style="background-image: url('/img/bg.png')">content</div>"#;
        let page_url = u("https://example.com/about/index.html");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/about/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("../img/bg.png"), "got: {}", result);
        assert!(result.contains("style=\""));
    }

    #[test]
    fn rewrite_html_inline_style_background_shorthand() {
        let html = r#"<section style="background: url('https://example.com/img/hero.jpg') no-repeat center"></section>"#;
        let page_url = u("https://example.com/index.html");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("img/hero.jpg"), "got: {}", result);
        assert!(result.contains("no-repeat center"));
    }

    #[test]
    fn rewrite_html_style_block() {
        let html = r#"<html><head><style>body { background: url("/img/bg.png"); } .hero { background-image: url("/img/hero.jpg"); }</style></head></html>"#;
        let page_url = u("https://example.com/about/index.html");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/about/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("../img/bg.png"), "got: {}", result);
        assert!(result.contains("../img/hero.jpg"), "got: {}", result);
    }

    #[test]
    fn rewrite_html_inline_style_external_url_unchanged() {
        let html = r#"<div style="background-image: url('https://cdn.other.com/img.png')">x</div>"#;
        let page_url = u("https://example.com/index.html");
        let out = Path::new("out");
        let page_fp = PathBuf::from("out/index.html");

        let result = rewrite_html(html, &page_url, "example.com", out, false, &empty_map(), &page_fp);
        assert!(result.contains("https://cdn.other.com/img.png"));
    }
}
