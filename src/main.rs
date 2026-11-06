mod config;
mod crawler;
mod downloader;
mod rewriter;
mod robots;
mod sitemap;
mod state;
mod warc;

use clap::Parser;
use config::Config;
use console::style;

#[tokio::main]
async fn main() {
    let config = Config::parse();

    if config.verbose && config.quiet {
        eprintln!(
            "{} Cannot use --verbose and --quiet together",
            style("Error:").red().bold()
        );
        std::process::exit(1);
    }

    let domain = url::Url::parse(&config.url)
        .map(|u| u.host_str().unwrap_or("unknown").to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let output = config.output.clone().unwrap_or_else(|| domain.clone());

    if !config.quiet {
        println!(
            "{} {} into {}/",
            style("Downloading").green().bold(),
            style(&config.url).cyan(),
            style(&output).cyan(),
        );
        if config.warc {
            println!("  WARC archive enabled");
        }
        if config.respect_robots {
            println!("  Respecting robots.txt");
        }
        if !config.exclude.is_empty() {
            println!("  Excluding: {}", config.exclude.join(", "));
        }
        if let Some(ref max) = config.max_size {
            println!("  Size limit: {}", max);
        }
    }

    match crawler::crawl(&config).await {
        Ok(stats) => {
            if !config.quiet {
                println!();
                println!("{}", style("Done!").green().bold());
                println!(
                    "  Pages:  {}  |  Assets: {}  |  Errors: {}",
                    style(stats.pages).cyan(),
                    style(stats.assets).cyan(),
                    style(stats.errors).yellow(),
                );
                let size = if stats.total_bytes > 1_048_576 {
                    format!("{:.1} MB", stats.total_bytes as f64 / 1_048_576.0)
                } else {
                    format!("{:.1} KB", stats.total_bytes as f64 / 1024.0)
                };
                println!("  Total:  {}", style(size).cyan());
                if stats.skipped_resume > 0 {
                    println!(
                        "  Skipped {} previously downloaded URLs (resume)",
                        style(stats.skipped_resume).dim()
                    );
                }
                println!(
                    "\n  Open {}/{} in your browser to view the site.",
                    output, "index.html"
                );
            }
        }
        Err(e) => {
            eprintln!("{} {}", style("Error:").red().bold(), e);
            std::process::exit(1);
        }
    }
}
