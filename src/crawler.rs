use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use globset::{Glob, GlobSet, GlobSetBuilder};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use scraper::{Html, Selector};
use tokio::sync::Mutex;
use url::Url;

use crate::config::{parse_size, Config};
use crate::downloader::download_url;
use crate::rewriter::{rewrite_css, rewrite_html};
use crate::robots::{fetch_robots, RobotsRules};
use crate::sitemap::fetch_sitemap_urls;
use crate::state::State;
use crate::warc::WarcWriter;

struct CrawlItem {
    url: Url,
    depth: u32,
}

pub struct CrawlStats {
    pub pages: u64,
    pub assets: u64,
    pub errors: u64,
    pub total_bytes: u64,
    pub skipped_resume: usize,
}

struct RewriteEntry {
    url: Url,
    filepath: PathBuf,
    is_css: bool,
}

pub async fn crawl(config: &Config) -> Result<CrawlStats, String> {
    let start_url =
        Url::parse(&config.url).map_err(|e| format!("Invalid URL '{}': {}", config.url, e))?;

    let base_domain = start_url
        .host_str()
        .ok_or("URL has no host")?
        .to_string();

    let output_dir = PathBuf::from(config.output.clone().unwrap_or_else(|| base_domain.clone()));

    std::fs::create_dir_all(&output_dir)
        .map_err(|e| format!("Failed to create output dir: {}", e))?;

    let client = Client::builder()
        .user_agent(&config.user_agent)
        .timeout(Duration::from_secs(config.timeout))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    // Build exclude patterns
    let excludes = build_excludes(&config.exclude)?;

    // Parse max size
    let max_bytes: Option<u64> = config
        .max_size
        .as_ref()
        .map(|s| parse_size(s))
        .transpose()?;

    // Load resume state
    let mut state = if config.no_resume {
        State::empty(&output_dir)
    } else {
        State::load(&output_dir)
    };
    let resumed_count = state.count();

    // Robots.txt
    let robots = if config.respect_robots {
        if !config.quiet {
            eprintln!("  Fetching robots.txt...");
        }
        fetch_robots(&client, &start_url, &config.user_agent).await
    } else {
        RobotsRules::allow_all()
    };

    // Sitemap
    let mut sitemap_urls = Vec::new();
    if !config.quiet {
        if config.verbose {
            eprintln!("  Checking sitemap.xml...");
        }
        sitemap_urls = fetch_sitemap_urls(&client, &start_url).await;
        if !sitemap_urls.is_empty() && config.verbose {
            eprintln!("  Found {} URLs in sitemap", sitemap_urls.len());
        }
    }

    // WARC writer
    let warc_writer = if config.warc {
        Some(Arc::new(Mutex::new(WarcWriter::new(
            &output_dir,
            &base_domain,
        )?)))
    } else {
        None
    };

    // Visited set and queue
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: VecDeque<CrawlItem> = VecDeque::new();

    // Seed with start URL
    let mut normalized_start = start_url.clone();
    normalized_start.set_fragment(None);
    visited.insert(normalized_start.as_str().to_string());
    queue.push_back(CrawlItem {
        url: normalized_start,
        depth: 0,
    });

    // Seed with sitemap URLs
    for surl in sitemap_urls {
        if let Some(normalized) =
            normalize_and_filter(&surl, &base_domain, config.include_subdomains)
        {
            let key = normalized.as_str().to_string();
            if !visited.contains(&key) {
                visited.insert(key);
                queue.push_back(CrawlItem {
                    url: normalized,
                    depth: 0,
                });
            }
        }
    }

    let total_bytes = Arc::new(AtomicU64::new(0));

    let mut stats = CrawlStats {
        pages: 0,
        assets: 0,
        errors: 0,
        total_bytes: 0,
        skipped_resume: 0,
    };

    // filepath map for rewriter: url_string -> actual filepath
    let mut filepath_map: HashMap<String, PathBuf> = HashMap::new();
    // Files that need rewriting (HTML/CSS)
    let mut rewrite_entries: Vec<RewriteEntry> = Vec::new();

    // Progress bar
    let pb = if config.quiet {
        ProgressBar::hidden()
    } else {
        let pb = ProgressBar::new(queue.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template(
                    "{spinner:.green} [{elapsed_precise}] [{bar:30.white/dim}] {pos}/{len} ({per_sec}) ETA {eta} | {msg}",
                )
                .unwrap()
                .progress_chars("=> "),
        );
        pb
    };

    // --- Phase 1: Download everything with true concurrency ---
    let mut in_flight = FuturesUnordered::new();

    loop {
        // Check size limit
        if let Some(max) = max_bytes {
            if total_bytes.load(Ordering::Relaxed) >= max {
                pb.println(format!(
                    "  {} Size limit reached ({})",
                    console::style("STOP").yellow(),
                    config.max_size.as_deref().unwrap_or("")
                ));
                break;
            }
        }

        // Fill up to concurrency limit
        while in_flight.len() < config.concurrency {
            if let Some(item) = queue.pop_front() {
                let url_str = item.url.as_str().to_string();

                // Skip if already done (resume)
                if state.is_done(&url_str) {
                    stats.skipped_resume += 1;
                    continue;
                }

                // Skip if depth exceeded
                if item.depth > config.depth {
                    continue;
                }

                // Check robots.txt
                if config.respect_robots && !robots.is_allowed(item.url.path()) {
                    if config.verbose {
                        pb.println(format!(
                            "  {} {} (robots.txt)",
                            console::style("SKIP").yellow(),
                            item.url
                        ));
                    }
                    continue;
                }

                // Check exclude patterns
                if is_excluded(item.url.path(), &excludes) {
                    if config.verbose {
                        pb.println(format!(
                            "  {} {} (excluded)",
                            console::style("SKIP").yellow(),
                            item.url
                        ));
                    }
                    continue;
                }

                let client = client.clone();
                let output_dir = output_dir.clone();
                let delay = config.delay;
                let total_bytes_clone = total_bytes.clone();
                let warc = warc_writer.clone();

                in_flight.push(tokio::spawn(async move {
                    // Rate limiting delay
                    if delay > 0 {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                    }

                    let result = download_url(&client, &item.url, &output_dir).await;

                    // Write WARC if enabled
                    if let (Some(warc), Ok(ref dl)) = (&warc, &result) {
                        let mut w = warc.lock().await;
                        let _ = w.write_response(
                            item.url.as_str(),
                            dl.status_code,
                            &dl.headers,
                            &dl.body,
                        );
                    }

                    if let Ok(ref dl) = result {
                        total_bytes_clone.fetch_add(dl.body.len() as u64, Ordering::Relaxed);
                    }

                    (item, result)
                }));
            } else {
                break;
            }
        }

        if in_flight.is_empty() {
            break;
        }

        // Wait for next completion
        let Some(join_result) = in_flight.next().await else {
            break;
        };

        let (item, result) = join_result.map_err(|e| format!("Task panicked: {}", e))?;

        match result {
            Ok(dl) => {
                let is_html = dl.content_type.contains("text/html");
                let is_css = dl.content_type.contains("text/css");

                // Store filepath mapping
                let url_key = item.url.as_str().to_string();
                filepath_map.insert(url_key.clone(), dl.filepath.clone());

                // Mark as done for resume
                state.mark_done(&url_key);

                if is_html {
                    stats.pages += 1;
                    let html_str = String::from_utf8_lossy(&dl.body);

                    // Extract links
                    let discovered = extract_links_from_html(&html_str, &item.url);
                    let new_count = enqueue_discovered(
                        discovered,
                        &base_domain,
                        config.include_subdomains,
                        config.assets_only,
                        item.depth,
                        &mut visited,
                        &mut queue,
                    );

                    pb.set_length(pb.length().unwrap_or(0) + new_count as u64);

                    rewrite_entries.push(RewriteEntry {
                        url: item.url.clone(),
                        filepath: dl.filepath,
                        is_css: false,
                    });
                } else if is_css {
                    stats.assets += 1;
                    let css_str = String::from_utf8_lossy(&dl.body);

                    let discovered = extract_links_from_css(&css_str, &item.url);
                    let new_count = enqueue_discovered(
                        discovered,
                        &base_domain,
                        config.include_subdomains,
                        false,
                        item.depth,
                        &mut visited,
                        &mut queue,
                    );

                    pb.set_length(pb.length().unwrap_or(0) + new_count as u64);

                    rewrite_entries.push(RewriteEntry {
                        url: item.url.clone(),
                        filepath: dl.filepath,
                        is_css: true,
                    });
                } else {
                    stats.assets += 1;
                }

                if config.verbose {
                    pb.println(format!(
                        "  {} {} ({})",
                        console::style("GET").green(),
                        item.url,
                        format_bytes(dl.body.len() as u64)
                    ));
                }
            }
            Err(e) => {
                stats.errors += 1;
                if !config.quiet {
                    pb.println(format!("  {} {}", console::style("ERR").red(), e));
                }
            }
        }

        stats.total_bytes = total_bytes.load(Ordering::Relaxed);
        pb.inc(1);
        pb.set_message(truncate_url(
            in_flight
                .iter()
                .count()
                .to_string()
                .as_str(),
            &format!(
                "{}p/{}a/{}e",
                stats.pages, stats.assets, stats.errors
            ),
        ));
    }

    pb.finish_and_clear();
    stats.total_bytes = total_bytes.load(Ordering::Relaxed);

    // --- Phase 2: Rewrite all HTML/CSS files ---
    if !config.quiet && !rewrite_entries.is_empty() {
        eprintln!(
            "  {} Rewriting {} files...",
            console::style(">>").dim(),
            rewrite_entries.len()
        );
    }

    for entry in &rewrite_entries {
        let content = std::fs::read_to_string(&entry.filepath).unwrap_or_default();

        let rewritten = if entry.is_css {
            rewrite_css(
                &content,
                &entry.url,
                &base_domain,
                &output_dir,
                config.include_subdomains,
                &filepath_map,
                &entry.filepath,
            )
        } else {
            rewrite_html(
                &content,
                &entry.url,
                &base_domain,
                &output_dir,
                config.include_subdomains,
                &filepath_map,
                &entry.filepath,
            )
        };

        let _ = std::fs::write(&entry.filepath, rewritten.as_bytes());
    }

    stats.skipped_resume = if resumed_count > 0 {
        resumed_count
    } else {
        stats.skipped_resume
    };

    Ok(stats)
}

fn enqueue_discovered(
    discovered: Vec<Url>,
    base_domain: &str,
    include_subdomains: bool,
    assets_only: bool,
    parent_depth: u32,
    visited: &mut HashSet<String>,
    queue: &mut VecDeque<CrawlItem>,
) -> usize {
    let mut count = 0;
    for link in discovered {
        if let Some(new_url) = normalize_and_filter(&link, base_domain, include_subdomains) {
            let key = new_url.as_str().to_string();
            if !visited.contains(&key) {
                visited.insert(key);
                let is_page = is_likely_page(&new_url);
                if assets_only && is_page {
                    continue;
                }
                queue.push_back(CrawlItem {
                    url: new_url,
                    depth: if is_page {
                        parent_depth + 1
                    } else {
                        parent_depth
                    },
                });
                count += 1;
            }
        }
    }
    count
}

fn truncate_url(active: &str, summary: &str) -> String {
    format!("{} active | {}", active, summary)
}

fn format_bytes(bytes: u64) -> String {
    if bytes > 1_048_576 {
        format!("{:.1}MB", bytes as f64 / 1_048_576.0)
    } else if bytes > 1024 {
        format!("{:.0}KB", bytes as f64 / 1024.0)
    } else {
        format!("{}B", bytes)
    }
}

fn build_excludes(patterns: &[String]) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|e| format!("Invalid exclude pattern '{}': {}", pattern, e))?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|e| format!("Failed to build exclude patterns: {}", e))
}

fn is_excluded(path: &str, excludes: &GlobSet) -> bool {
    if excludes.is_empty() {
        return false;
    }
    excludes.is_match(path)
}

fn extract_links_from_html(html: &str, base_url: &Url) -> Vec<Url> {
    let document = Html::parse_document(html);
    let mut urls = Vec::new();

    let selectors_attrs = [
        ("a[href]", "href"),
        ("link[href]", "href"),
        ("script[src]", "src"),
        ("img[src]", "src"),
        ("img[srcset]", "srcset"),
        ("img[data-src]", "data-src"),
        ("img[data-lazy-src]", "data-lazy-src"),
        ("img[data-srcset]", "data-srcset"),
        ("img[data-lazy-srcset]", "data-lazy-srcset"),
        ("img[data-original]", "data-original"),
        ("source[src]", "src"),
        ("source[srcset]", "srcset"),
        ("source[data-srcset]", "data-srcset"),
        ("video[src]", "src"),
        ("video[poster]", "poster"),
        ("video[data-src]", "data-src"),
        ("audio[src]", "src"),
        ("iframe[src]", "src"),
        ("iframe[data-src]", "data-src"),
        ("object[data]", "data"),
        ("embed[src]", "src"),
        ("[data-bg]", "data-bg"),
        ("[data-background-image]", "data-background-image"),
    ];

    for (sel_str, attr) in &selectors_attrs {
        if let Ok(selector) = Selector::parse(sel_str) {
            for element in document.select(&selector) {
                if let Some(value) = element.value().attr(attr) {
                    if attr.contains("srcset") {
                        for entry in value.split(',') {
                            let url_part = entry.trim().split_whitespace().next().unwrap_or("");
                            if let Ok(resolved) = base_url.join(url_part) {
                                urls.push(resolved);
                            }
                        }
                    } else if let Ok(resolved) = base_url.join(value) {
                        urls.push(resolved);
                    }
                }
            }
        }
    }

    // Inline style attributes
    let style_re = regex::Regex::new(r#"url\(\s*['"]?([^)'"]+)['"]?\s*\)"#).unwrap();
    if let Ok(selector) = Selector::parse("[style]") {
        for element in document.select(&selector) {
            if let Some(style) = element.value().attr("style") {
                for cap in style_re.captures_iter(style) {
                    if let Ok(resolved) = base_url.join(&cap[1]) {
                        urls.push(resolved);
                    }
                }
            }
        }
    }

    // <style> blocks
    if let Ok(selector) = Selector::parse("style") {
        for element in document.select(&selector) {
            let css_text = element.text().collect::<String>();
            urls.extend(extract_links_from_css(&css_text, base_url));
        }
    }

    urls
}

fn extract_links_from_css(css: &str, base_url: &Url) -> Vec<Url> {
    let url_re = regex::Regex::new(r#"url\(\s*['"]?([^)'"]+)['"]?\s*\)"#).unwrap();
    let import_re = regex::Regex::new(r#"@import\s+['"]([^'"]+)['"]"#).unwrap();
    let mut urls = Vec::new();

    for cap in url_re.captures_iter(css) {
        let raw = cap[1].trim();
        if !raw.starts_with("data:") {
            if let Ok(resolved) = base_url.join(raw) {
                urls.push(resolved);
            }
        }
    }

    for cap in import_re.captures_iter(css) {
        if let Ok(resolved) = base_url.join(&cap[1]) {
            urls.push(resolved);
        }
    }

    urls
}

fn normalize_and_filter(url: &Url, base_domain: &str, include_subdomains: bool) -> Option<Url> {
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }

    let host = url.host_str()?;

    let is_same_site = if include_subdomains {
        host == base_domain || host.ends_with(&format!(".{}", base_domain))
    } else {
        host == base_domain
    };

    if !is_same_site {
        return None;
    }

    let mut clean = url.clone();
    clean.set_fragment(None);
    Some(clean)
}

fn is_likely_page(url: &Url) -> bool {
    let path = url.path();
    if path.ends_with('/') || path.is_empty() {
        return true;
    }
    let last_segment = path.rsplit('/').next().unwrap_or(path);
    if !last_segment.contains('.') {
        return true;
    }
    let ext = last_segment.rsplit('.').next().unwrap_or("").to_lowercase();
    matches!(
        ext.as_str(),
        "html" | "htm" | "php" | "asp" | "aspx" | "jsp" | ""
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    // --- is_likely_page ---

    #[test]
    fn page_root() {
        assert!(is_likely_page(&u("https://example.com/")));
    }

    #[test]
    fn page_no_extension() {
        assert!(is_likely_page(&u("https://example.com/about")));
    }

    #[test]
    fn page_html_extension() {
        assert!(is_likely_page(&u("https://example.com/page.html")));
    }

    #[test]
    fn page_htm_extension() {
        assert!(is_likely_page(&u("https://example.com/page.htm")));
    }

    #[test]
    fn page_php_extension() {
        assert!(is_likely_page(&u("https://example.com/page.php")));
    }

    #[test]
    fn not_page_css() {
        assert!(!is_likely_page(&u("https://example.com/style.css")));
    }

    #[test]
    fn not_page_js() {
        assert!(!is_likely_page(&u("https://example.com/app.js")));
    }

    #[test]
    fn not_page_image() {
        assert!(!is_likely_page(&u("https://example.com/logo.png")));
    }

    #[test]
    fn not_page_font() {
        assert!(!is_likely_page(&u("https://example.com/font.woff2")));
    }

    // --- normalize_and_filter ---

    #[test]
    fn normalize_same_domain() {
        let url = u("https://example.com/page#section");
        let result = normalize_and_filter(&url, "example.com", false);
        assert!(result.is_some());
        let clean = result.unwrap();
        assert_eq!(clean.as_str(), "https://example.com/page");
        assert!(clean.fragment().is_none());
    }

    #[test]
    fn normalize_different_domain() {
        let url = u("https://other.com/page");
        assert!(normalize_and_filter(&url, "example.com", false).is_none());
    }

    #[test]
    fn normalize_subdomain_without_flag() {
        let url = u("https://blog.example.com/page");
        assert!(normalize_and_filter(&url, "example.com", false).is_none());
    }

    #[test]
    fn normalize_subdomain_with_flag() {
        let url = u("https://blog.example.com/page");
        assert!(normalize_and_filter(&url, "example.com", true).is_some());
    }

    #[test]
    fn normalize_ftp_rejected() {
        let url = u("ftp://example.com/file");
        assert!(normalize_and_filter(&url, "example.com", false).is_none());
    }

    #[test]
    fn normalize_mailto_rejected() {
        // mailto doesn't have a host, so it fails at host_str()
        let url = Url::parse("mailto:test@example.com");
        if let Ok(url) = url {
            assert!(normalize_and_filter(&url, "example.com", false).is_none());
        }
    }

    // --- extract_links_from_html ---

    #[test]
    fn extract_html_a_href() {
        let html = r#"<html><body><a href="/about">About</a></body></html>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/about"));
    }

    #[test]
    fn extract_html_link_href() {
        let html = r#"<html><head><link href="/css/style.css" rel="stylesheet"></head></html>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/css/style.css"));
    }

    #[test]
    fn extract_html_script_src() {
        let html = r#"<html><body><script src="/js/app.js"></script></body></html>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/js/app.js"));
    }

    #[test]
    fn extract_html_img_src() {
        let html = r#"<html><body><img src="/img/photo.jpg"></body></html>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/photo.jpg"));
    }

    #[test]
    fn extract_html_img_srcset() {
        let html = r#"<img srcset="/img/a.jpg 1x, /img/b.jpg 2x">"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/a.jpg"));
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/b.jpg"));
    }

    #[test]
    fn extract_html_video_poster() {
        let html = r#"<video poster="/img/thumb.jpg" src="/vid/clip.mp4"></video>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/thumb.jpg"));
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/vid/clip.mp4"));
    }

    // --- Lazy loading attributes ---

    #[test]
    fn extract_html_data_src() {
        let html = r#"<img data-src="/img/lazy.jpg" src="placeholder.gif">"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/lazy.jpg"));
    }

    #[test]
    fn extract_html_data_lazy_src() {
        let html = r#"<img data-lazy-src="/wp-content/uploads/2023/01/photo.png" src="/wp-content/uploads/2023/01/photo.png">"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/wp-content/uploads/2023/01/photo.png"));
    }

    #[test]
    fn extract_html_data_srcset() {
        let html = r#"<img data-srcset="/img/a.jpg 1x, /img/b.jpg 2x">"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/a.jpg"));
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/b.jpg"));
    }

    #[test]
    fn extract_html_data_lazy_srcset() {
        let html = r#"<img data-lazy-srcset="/img/a.jpg 300w, /img/b.jpg 600w">"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/a.jpg"));
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/b.jpg"));
    }

    #[test]
    fn extract_html_data_original() {
        let html = r#"<img data-original="/img/original.jpg">"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/original.jpg"));
    }

    #[test]
    fn extract_html_data_bg() {
        let html = r#"<div data-bg="/img/hero-bg.jpg"></div>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/hero-bg.jpg"));
    }

    #[test]
    fn extract_html_data_background_image() {
        let html = r#"<section data-background-image="/img/section-bg.jpg"></section>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/section-bg.jpg"));
    }

    #[test]
    fn extract_html_iframe_data_src() {
        let html = r#"<iframe data-src="/embed/video"></iframe>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/embed/video"));
    }

    #[test]
    fn extract_html_inline_style_url() {
        let html = r#"<div style="background: url('/img/bg.png')">hi</div>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/bg.png"));
    }

    #[test]
    fn extract_html_style_block() {
        let html = r#"<html><head><style>body { background: url("/img/bg.png"); }</style></head></html>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/bg.png"));
    }

    #[test]
    fn extract_html_absolute_url() {
        let html = r#"<a href="https://example.com/page">Page</a>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/page"));
    }

    #[test]
    fn extract_html_external_url() {
        let html = r#"<a href="https://other.com/page">Other</a>"#;
        let base = u("https://example.com/");
        let links = extract_links_from_html(html, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://other.com/page"));
    }

    // --- extract_links_from_css ---

    #[test]
    fn extract_css_url() {
        let css = r#"body { background: url("/img/bg.png"); }"#;
        let base = u("https://example.com/css/style.css");
        let links = extract_links_from_css(css, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/bg.png"));
    }

    #[test]
    fn extract_css_url_no_quotes() {
        let css = r#"body { background: url(/img/bg.png); }"#;
        let base = u("https://example.com/css/style.css");
        let links = extract_links_from_css(css, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/img/bg.png"));
    }

    #[test]
    fn extract_css_import() {
        let css = r#"@import "reset.css";"#;
        let base = u("https://example.com/css/style.css");
        let links = extract_links_from_css(css, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/css/reset.css"));
    }

    #[test]
    fn extract_css_import_single_quotes() {
        let css = r#"@import 'reset.css';"#;
        let base = u("https://example.com/css/style.css");
        let links = extract_links_from_css(css, &base);
        assert!(links.iter().any(|l| l.as_str() == "https://example.com/css/reset.css"));
    }

    #[test]
    fn extract_css_data_uri_skipped() {
        let css = r#"body { background: url("data:image/png;base64,abc"); }"#;
        let base = u("https://example.com/css/style.css");
        let links = extract_links_from_css(css, &base);
        assert!(links.is_empty());
    }

    #[test]
    fn extract_css_multiple_urls() {
        let css = r#"
            body { background: url("/img/bg.png"); }
            @font-face { src: url("/fonts/roboto.woff2"); }
            @import "/css/reset.css";
        "#;
        let base = u("https://example.com/css/style.css");
        let links = extract_links_from_css(css, &base);
        assert!(links.len() >= 3);
    }

    // --- build_excludes / is_excluded ---

    #[test]
    fn exclude_glob_pattern() {
        let excludes = build_excludes(&["*.pdf".to_string()]).unwrap();
        assert!(is_excluded("/docs/manual.pdf", &excludes));
        assert!(!is_excluded("/docs/manual.html", &excludes));
    }

    #[test]
    fn exclude_path_pattern() {
        let excludes = build_excludes(&["/admin/*".to_string()]).unwrap();
        assert!(is_excluded("/admin/settings", &excludes));
        assert!(!is_excluded("/about", &excludes));
    }

    #[test]
    fn exclude_multiple_patterns() {
        let excludes = build_excludes(&[
            "*.pdf".to_string(),
            "/admin/*".to_string(),
        ]).unwrap();
        assert!(is_excluded("/docs/file.pdf", &excludes));
        assert!(is_excluded("/admin/page", &excludes));
        assert!(!is_excluded("/about.html", &excludes));
    }

    #[test]
    fn exclude_empty_patterns() {
        let excludes = build_excludes(&[]).unwrap();
        assert!(!is_excluded("/anything", &excludes));
    }

    // --- format_bytes ---

    #[test]
    fn format_bytes_small() {
        assert_eq!(format_bytes(500), "500B");
    }

    #[test]
    fn format_bytes_kb() {
        assert_eq!(format_bytes(5120), "5KB");
    }

    #[test]
    fn format_bytes_mb() {
        assert_eq!(format_bytes(2_621_440), "2.5MB");
    }

    // --- enqueue_discovered ---

    #[test]
    fn enqueue_filters_external() {
        let links = vec![
            u("https://example.com/page1"),
            u("https://other.com/page2"),
            u("https://example.com/page3"),
        ];
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        let count = enqueue_discovered(links, "example.com", false, false, 0, &mut visited, &mut queue);
        assert_eq!(count, 2);
        assert_eq!(queue.len(), 2);
    }

    #[test]
    fn enqueue_deduplicates() {
        let links = vec![
            u("https://example.com/page1"),
            u("https://example.com/page1"),
            u("https://example.com/page1"),
        ];
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        let count = enqueue_discovered(links, "example.com", false, false, 0, &mut visited, &mut queue);
        assert_eq!(count, 1);
    }

    #[test]
    fn enqueue_skips_pages_in_assets_only() {
        let links = vec![
            u("https://example.com/page"),
            u("https://example.com/style.css"),
            u("https://example.com/logo.png"),
        ];
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        let count = enqueue_discovered(links, "example.com", false, true, 0, &mut visited, &mut queue);
        assert_eq!(count, 2); // only CSS and PNG
    }

    #[test]
    fn enqueue_increments_depth_for_pages() {
        let links = vec![
            u("https://example.com/page.html"),
            u("https://example.com/style.css"),
        ];
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        enqueue_discovered(links, "example.com", false, false, 3, &mut visited, &mut queue);
        let depths: Vec<u32> = queue.iter().map(|i| i.depth).collect();
        assert!(depths.contains(&4)); // page depth incremented
        assert!(depths.contains(&3)); // asset depth stays
    }

    #[test]
    fn enqueue_strips_fragments() {
        let links = vec![
            u("https://example.com/page#top"),
            u("https://example.com/page#bottom"),
        ];
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        let count = enqueue_discovered(links, "example.com", false, false, 0, &mut visited, &mut queue);
        // Both should normalize to the same URL
        assert_eq!(count, 1);
    }
}
