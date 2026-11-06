# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [2.1.0] - 2026-03-06

### Added
- Support for lazy-loading image attributes (`data-src`, `data-lazy-src`, `data-srcset`, `data-lazy-srcset`, `data-original`, `data-bg`, `data-background-image`)
- Rewriting of `url()` references in inline `style` attributes
- Rewriting of `url()` references in `<style>` blocks within HTML
- 19 new unit tests for lazy-loading and inline style handling

### Fixed
- Images on WordPress sites using lazy-loading plugins now download and display correctly
- `data-srcset` and `data-lazy-srcset` attributes are now properly parsed as srcset values

## [2.0.0] - 2026-03-06

### Added
- True concurrent downloads using `FuturesUnordered`
- `robots.txt` support with `--respect-robots` flag (off by default)
- Sitemap XML parsing with recursive sitemap index support
- WARC/1.0 archive output with `--warc` flag
- Resume interrupted crawls (append-only `.sitegrabber-state` file)
- `--no-resume` flag to force re-download
- Glob-based URL exclude patterns with `--exclude`
- Rate limiting with `--delay` (default 100ms)
- Max file size filtering with `--max-size`
- CSS `@import` rewriting
- Progress bar with ETA using indicatif
- `--verbose` and `--quiet` modes
- MIME-based file extension detection
- Full test suite (145+ unit tests, 12 integration tests)

## [1.0.0] - 2026-03-06

### Added
- Initial release
- Async website crawler with configurable depth and concurrency
- HTML link extraction (`<a>`, `<link>`, `<script>`, `<img>`, `<source>`, `<video>`, etc.)
- CSS `url()` extraction and rewriting
- Relative path rewriting for offline browsing
- Configurable user-agent, timeout, output directory
- `--include-subdomains` and `--assets-only` modes
