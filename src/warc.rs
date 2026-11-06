use chrono::Utc;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

pub struct WarcWriter {
    file: File,
}

impl WarcWriter {
    pub fn new(output_dir: &Path, domain: &str) -> Result<Self, String> {
        let timestamp = Utc::now().format("%Y%m%d%H%M%S");
        let filename = format!("{}-{}.warc", domain, timestamp);
        let path = output_dir.join(&filename);

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| format!("Failed to create WARC file {:?}: {}", path, e))?;

        // Write warcinfo record
        let mut writer = WarcWriter { file };
        writer.write_warcinfo(domain)?;
        Ok(writer)
    }

    fn write_warcinfo(&mut self, domain: &str) -> Result<(), String> {
        let date = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let record_id = generate_record_id();
        let payload = format!("software: SiteGrabber/2.0\r\nhost: {}\r\n", domain);

        write!(
            self.file,
            "WARC/1.0\r\n\
             WARC-Type: warcinfo\r\n\
             WARC-Date: {}\r\n\
             WARC-Record-ID: <urn:uuid:{}>\r\n\
             Content-Type: application/warc-fields\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {}\r\n\r\n",
            date,
            record_id,
            payload.len(),
            payload,
        )
        .map_err(|e| format!("Failed to write WARC info: {}", e))
    }

    pub fn write_response(
        &mut self,
        target_uri: &str,
        status_code: u16,
        response_headers: &[(String, String)],
        body: &[u8],
    ) -> Result<(), String> {
        let date = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let record_id = generate_record_id();

        // Build HTTP response block
        let mut http_response = format!("HTTP/1.1 {} OK\r\n", status_code);
        for (key, value) in response_headers {
            http_response.push_str(&format!("{}: {}\r\n", key, value));
        }
        http_response.push_str("\r\n");

        let payload_len = http_response.len() + body.len();

        // Write WARC headers
        write!(
            self.file,
            "WARC/1.0\r\n\
             WARC-Type: response\r\n\
             WARC-Date: {}\r\n\
             WARC-Target-URI: {}\r\n\
             WARC-Record-ID: <urn:uuid:{}>\r\n\
             Content-Type: application/http;msgtype=response\r\n\
             Content-Length: {}\r\n\
             \r\n\
             {}",
            date, target_uri, record_id, payload_len, http_response,
        )
        .map_err(|e| format!("WARC write error: {}", e))?;

        self.file
            .write_all(body)
            .map_err(|e| format!("WARC write error: {}", e))?;

        write!(self.file, "\r\n\r\n").map_err(|e| format!("WARC write error: {}", e))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn tmp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "sitegrabber_warc_test_{}_{}",
            std::process::id(), id
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn warc_creates_file() {
        let dir = tmp_dir();
        let writer = WarcWriter::new(&dir, "example.com");
        assert!(writer.is_ok());

        let files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "warc").unwrap_or(false))
            .collect();
        assert_eq!(files.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn warc_contains_warcinfo() {
        let dir = tmp_dir();
        let _writer = WarcWriter::new(&dir, "example.com").unwrap();

        let files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|x| x == "warc").unwrap_or(false))
            .collect();
        assert_eq!(files.len(), 1);
        let content = fs::read_to_string(files[0].path()).unwrap();
        assert!(content.contains("WARC/1.0"));
        assert!(content.contains("WARC-Type: warcinfo"));
        assert!(content.contains("SiteGrabber/2.0"));
        assert!(content.contains("example.com"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn warc_write_response() {
        let dir = tmp_dir();
        let mut writer = WarcWriter::new(&dir, "example.com").unwrap();

        let headers = vec![
            ("content-type".to_string(), "text/html".to_string()),
            ("content-length".to_string(), "13".to_string()),
        ];
        let body = b"<html>hi</html>";

        let result = writer.write_response(
            "https://example.com/",
            200,
            &headers,
            body,
        );
        assert!(result.is_ok());

        let files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        let content = fs::read_to_string(files[0].path()).unwrap();
        assert!(content.contains("WARC-Type: response"));
        assert!(content.contains("WARC-Target-URI: https://example.com/"));
        assert!(content.contains("HTTP/1.1 200 OK"));
        assert!(content.contains("<html>hi</html>"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn warc_multiple_responses() {
        let dir = tmp_dir();
        let mut writer = WarcWriter::new(&dir, "example.com").unwrap();

        for i in 0..3 {
            writer
                .write_response(
                    &format!("https://example.com/page{}", i),
                    200,
                    &[],
                    b"body",
                )
                .unwrap();
        }

        let files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        let content = fs::read_to_string(files[0].path()).unwrap();
        // 1 warcinfo + 3 responses = 4 WARC records
        assert_eq!(content.matches("WARC/1.0").count(), 4);
        let _ = fs::remove_dir_all(&dir);
    }
}

fn generate_record_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Simple pseudo-UUID from timestamp + random-ish bits
    let a = (nanos & 0xFFFFFFFF) as u32;
    let b = ((nanos >> 32) & 0xFFFF) as u16;
    let c = ((nanos >> 48) & 0x0FFF) as u16 | 0x4000;
    let d = ((nanos >> 60) & 0x3FFF) as u16 | 0x8000;
    let e = (nanos.wrapping_mul(6364136223846793005) & 0xFFFFFFFFFFFF) as u64;
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        a,
        b,
        c,
        d,
        e & 0xFFFFFFFFFFFF
    )
}
