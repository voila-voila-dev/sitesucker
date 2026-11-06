use clap::Parser;

#[derive(Parser, Debug, Clone)]
#[command(name = "sitegrabber", about = "Download an entire website for offline browsing")]
pub struct Config {
    /// The URL to download
    pub url: String,

    /// Output directory (default: domain name)
    #[arg(short, long)]
    pub output: Option<String>,

    /// Max crawl depth
    #[arg(short, long, default_value_t = 25)]
    pub depth: u32,

    /// Parallel downloads
    #[arg(short, long, default_value_t = 8)]
    pub concurrency: usize,

    /// Request timeout in seconds
    #[arg(short, long, default_value_t = 30)]
    pub timeout: u64,

    /// Custom user-agent
    #[arg(long, default_value = "SiteGrabber/1.0")]
    pub user_agent: String,

    /// Also crawl subdomains
    #[arg(long)]
    pub include_subdomains: bool,

    /// Only download assets, don't crawl pages
    #[arg(long)]
    pub assets_only: bool,

    /// Delay between requests in milliseconds (politeness)
    #[arg(long, default_value_t = 100)]
    pub delay: u64,

    /// Respect robots.txt (default: ignored)
    #[arg(long)]
    pub respect_robots: bool,

    /// Exclude URL patterns (glob syntax, can be repeated)
    #[arg(long)]
    pub exclude: Vec<String>,

    /// Max total download size (e.g. "50MB", "1GB")
    #[arg(long)]
    pub max_size: Option<String>,

    /// Save a WARC archive alongside the files
    #[arg(long)]
    pub warc: bool,

    /// Verbose output (log every request)
    #[arg(short, long)]
    pub verbose: bool,

    /// Quiet mode (only errors)
    #[arg(short, long)]
    pub quiet: bool,

    /// Disable resume (don't use state file)
    #[arg(long)]
    pub no_resume: bool,
}

pub fn parse_size(s: &str) -> Result<u64, String> {
    let s = s.trim().to_uppercase();
    let (num_str, multiplier) = if s.ends_with("GB") {
        (&s[..s.len() - 2], 1_073_741_824u64)
    } else if s.ends_with("MB") {
        (&s[..s.len() - 2], 1_048_576u64)
    } else if s.ends_with("KB") {
        (&s[..s.len() - 2], 1_024u64)
    } else if s.ends_with('B') {
        (&s[..s.len() - 1], 1u64)
    } else {
        (s.as_str(), 1u64)
    };
    let num: f64 = num_str
        .trim()
        .parse()
        .map_err(|_| format!("Invalid size: {}", s))?;
    Ok((num * multiplier as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_megabytes() {
        assert_eq!(parse_size("50MB").unwrap(), 52_428_800);
    }

    #[test]
    fn parse_size_gigabytes() {
        assert_eq!(parse_size("1GB").unwrap(), 1_073_741_824);
    }

    #[test]
    fn parse_size_kilobytes() {
        assert_eq!(parse_size("512KB").unwrap(), 524_288);
    }

    #[test]
    fn parse_size_bytes() {
        assert_eq!(parse_size("1024B").unwrap(), 1024);
    }

    #[test]
    fn parse_size_plain_number() {
        assert_eq!(parse_size("4096").unwrap(), 4096);
    }

    #[test]
    fn parse_size_decimal() {
        assert_eq!(parse_size("1.5GB").unwrap(), 1_610_612_736);
    }

    #[test]
    fn parse_size_with_whitespace() {
        assert_eq!(parse_size("  50MB  ").unwrap(), 52_428_800);
    }

    #[test]
    fn parse_size_lowercase() {
        assert_eq!(parse_size("50mb").unwrap(), 52_428_800);
    }

    #[test]
    fn parse_size_invalid() {
        assert!(parse_size("abc").is_err());
        assert!(parse_size("MB").is_err());
    }
}
