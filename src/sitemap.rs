use quick_xml::events::Event;
use quick_xml::reader::Reader;
use reqwest::Client;
use url::Url;

pub async fn fetch_sitemap_urls(client: &Client, base_url: &Url) -> Vec<Url> {
    let mut urls = Vec::new();
    let sitemap_url = format!(
        "{}://{}/sitemap.xml",
        base_url.scheme(),
        base_url.host_str().unwrap_or("")
    );

    if let Ok(parsed) = Url::parse(&sitemap_url) {
        fetch_sitemap_recursive(client, &parsed, &mut urls, 0).await;
    }

    urls
}

async fn fetch_sitemap_recursive(client: &Client, url: &Url, urls: &mut Vec<Url>, depth: u32) {
    if depth > 3 {
        return;
    }

    let body = match client.get(url.as_str()).send().await {
        Ok(resp) if resp.status().is_success() => resp.text().await.unwrap_or_default(),
        _ => return,
    };

    let mut reader = Reader::from_str(&body);
    let mut in_loc = false;
    let mut in_sitemap = false;
    let mut buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "loc" => {
                        in_loc = true;
                        buf.clear();
                    }
                    "sitemap" => {
                        in_sitemap = true;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(e)) if in_loc => {
                buf.push_str(&e.unescape().unwrap_or_default());
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "loc" && in_loc {
                    in_loc = false;
                    let trimmed = buf.trim();
                    if let Ok(parsed) = Url::parse(trimmed) {
                        if in_sitemap {
                            // This is a sitemap index entry — recurse
                            Box::pin(fetch_sitemap_recursive(client, &parsed, urls, depth + 1))
                                .await;
                        } else {
                            urls.push(parsed);
                        }
                    }
                } else if name == "sitemap" {
                    in_sitemap = false;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
}
