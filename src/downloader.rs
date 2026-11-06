use std::path::{Path, PathBuf};
use reqwest::Client;
use url::Url;

pub struct DownloadResult {
    pub body: Vec<u8>,
    pub content_type: String,
    pub filepath: PathBuf,
    pub status_code: u16,
    pub headers: Vec<(String, String)>,
}

/// Guess a file extension from a MIME content-type.
fn mime_to_extension(content_type: &str) -> Option<&'static str> {
    let mime = content_type.split(';').next().unwrap_or("").trim();
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" | "image/jpg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/svg+xml" => Some("svg"),
        "image/webp" => Some("webp"),
        "image/avif" => Some("avif"),
        "image/x-icon" | "image/vnd.microsoft.icon" => Some("ico"),
        "application/javascript" | "text/javascript" => Some("js"),
        "text/css" => Some("css"),
        "text/html" => Some("html"),
        "application/json" => Some("json"),
        "application/xml" | "text/xml" => Some("xml"),
        "application/pdf" => Some("pdf"),
        "font/woff" => Some("woff"),
        "font/woff2" | "application/font-woff2" => Some("woff2"),
        "font/ttf" | "application/x-font-ttf" => Some("ttf"),
        "font/otf" => Some("otf"),
        "application/font-woff" => Some("woff"),
        "video/mp4" => Some("mp4"),
        "video/webm" => Some("webm"),
        "audio/mpeg" => Some("mp3"),
        "audio/ogg" => Some("ogg"),
        _ => None,
    }
}

/// Map a URL to a filesystem path inside the output directory.
/// If content_type is provided and the path has no extension, add one based on MIME.
pub fn url_to_filepath(url: &Url, output_dir: &Path, content_type: Option<&str>) -> PathBuf {
    let mut path = url.path().to_string();

    // Strip leading slash
    if path.starts_with('/') {
        path = path[1..].to_string();
    }

    // If path is empty or ends with '/', treat as index.html
    if path.is_empty() || path.ends_with('/') {
        path.push_str("index.html");
    } else {
        let last_segment = path.rsplit('/').next().unwrap_or(&path);
        if !last_segment.contains('.') {
            // No extension — check MIME type
            if let Some(ct) = content_type {
                if ct.contains("text/html") {
                    path.push_str("/index.html");
                } else if let Some(ext) = mime_to_extension(ct) {
                    path.push_str(&format!(".{}", ext));
                } else {
                    path.push_str("/index.html");
                }
            } else {
                path.push_str("/index.html");
            }
        }
    }

    // Include query string in filename to differentiate
    if let Some(query) = url.query() {
        let ext_pos = path.rfind('.');
        if let Some(pos) = ext_pos {
            let sanitized_query = query.replace(['&', '=', '?', '/', '\\'], "_");
            path.insert_str(pos, &format!("_{}", sanitized_query));
        }
    }

    output_dir.join(&path)
}

/// For the rewriter: compute filepath without knowing content-type (fallback to index.html).
pub fn url_to_filepath_guess(url: &Url, output_dir: &Path) -> PathBuf {
    url_to_filepath(url, output_dir, None)
}

/// Download a URL and save it to disk.
pub async fn download_url(
    client: &Client,
    url: &Url,
    output_dir: &Path,
) -> Result<DownloadResult, String> {
    let response = client
        .get(url.as_str())
        .send()
        .await
        .map_err(|e| format!("Failed to fetch {}: {}", url, e))?;

    if !response.status().is_success() {
        return Err(format!("HTTP {} for {}", response.status(), url));
    }

    let status_code = response.status().as_u16();

    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read body for {}: {}", url, e))?;

    let filepath = url_to_filepath(url, output_dir, Some(&content_type));

    // Create parent directories
    if let Some(parent) = filepath.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create dir {:?}: {}", parent, e))?;
    }

    std::fs::write(&filepath, &bytes)
        .map_err(|e| format!("Failed to write {:?}: {}", filepath, e))?;

    Ok(DownloadResult {
        body: bytes.to_vec(),
        content_type,
        filepath,
        status_code,
        headers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    // --- url_to_filepath ---

    #[test]
    fn filepath_root_url() {
        let p = url_to_filepath(&u("https://example.com/"), Path::new("out"), None);
        assert_eq!(p, PathBuf::from("out/index.html"));
    }

    #[test]
    fn filepath_root_no_trailing_slash() {
        let p = url_to_filepath(&u("https://example.com"), Path::new("out"), None);
        assert_eq!(p, PathBuf::from("out/index.html"));
    }

    #[test]
    fn filepath_html_file() {
        let p = url_to_filepath(&u("https://example.com/about.html"), Path::new("out"), None);
        assert_eq!(p, PathBuf::from("out/about.html"));
    }

    #[test]
    fn filepath_extensionless_path_no_mime() {
        let p = url_to_filepath(&u("https://example.com/about"), Path::new("out"), None);
        assert_eq!(p, PathBuf::from("out/about/index.html"));
    }

    #[test]
    fn filepath_extensionless_path_html_mime() {
        let p = url_to_filepath(
            &u("https://example.com/about"),
            Path::new("out"),
            Some("text/html; charset=utf-8"),
        );
        assert_eq!(p, PathBuf::from("out/about/index.html"));
    }

    #[test]
    fn filepath_extensionless_path_image_mime() {
        let p = url_to_filepath(
            &u("https://example.com/img/logo"),
            Path::new("out"),
            Some("image/png"),
        );
        assert_eq!(p, PathBuf::from("out/img/logo.png"));
    }

    #[test]
    fn filepath_extensionless_path_js_mime() {
        let p = url_to_filepath(
            &u("https://example.com/assets/app"),
            Path::new("out"),
            Some("application/javascript"),
        );
        assert_eq!(p, PathBuf::from("out/assets/app.js"));
    }

    #[test]
    fn filepath_extensionless_path_unknown_mime() {
        let p = url_to_filepath(
            &u("https://example.com/data/blob"),
            Path::new("out"),
            Some("application/octet-stream"),
        );
        assert_eq!(p, PathBuf::from("out/data/blob/index.html"));
    }

    #[test]
    fn filepath_nested_path() {
        let p = url_to_filepath(
            &u("https://example.com/blog/2024/my-post"),
            Path::new("out"),
            None,
        );
        assert_eq!(p, PathBuf::from("out/blog/2024/my-post/index.html"));
    }

    #[test]
    fn filepath_with_extension() {
        let p = url_to_filepath(
            &u("https://example.com/css/style.css"),
            Path::new("out"),
            None,
        );
        assert_eq!(p, PathBuf::from("out/css/style.css"));
    }

    #[test]
    fn filepath_trailing_slash() {
        let p = url_to_filepath(
            &u("https://example.com/about/"),
            Path::new("out"),
            None,
        );
        assert_eq!(p, PathBuf::from("out/about/index.html"));
    }

    #[test]
    fn filepath_with_query_string() {
        let p = url_to_filepath(
            &u("https://example.com/page.html?v=2&lang=en"),
            Path::new("out"),
            None,
        );
        let s = p.to_string_lossy().to_string();
        assert!(s.contains("page_v_2_lang_en.html"));
    }

    #[test]
    fn filepath_guess_matches_no_mime() {
        let url = u("https://example.com/about");
        let a = url_to_filepath(&url, Path::new("out"), None);
        let b = url_to_filepath_guess(&url, Path::new("out"));
        assert_eq!(a, b);
    }

    // --- mime_to_extension ---

    #[test]
    fn mime_common_types() {
        assert_eq!(mime_to_extension("image/png"), Some("png"));
        assert_eq!(mime_to_extension("image/jpeg"), Some("jpg"));
        assert_eq!(mime_to_extension("text/css"), Some("css"));
        assert_eq!(mime_to_extension("text/html"), Some("html"));
        assert_eq!(mime_to_extension("application/javascript"), Some("js"));
        assert_eq!(mime_to_extension("font/woff2"), Some("woff2"));
        assert_eq!(mime_to_extension("application/pdf"), Some("pdf"));
        assert_eq!(mime_to_extension("video/mp4"), Some("mp4"));
    }

    #[test]
    fn mime_with_charset() {
        assert_eq!(
            mime_to_extension("text/html; charset=utf-8"),
            Some("html")
        );
    }

    #[test]
    fn mime_unknown() {
        assert_eq!(mime_to_extension("application/octet-stream"), None);
        assert_eq!(mime_to_extension("x-custom/whatever"), None);
    }
}
