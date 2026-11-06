# sitegrabber

A fast, single-binary CLI tool that downloads entire websites for offline browsing. Give it a URL and it crawls every page, grabs all the assets (CSS, JS, images, fonts), rewrites links to relative paths, and saves everything into a clean local directory you can open in a browser or deploy to static hosting.

## Install

```sh
curl -fsSL https://sitegrabber.dev/install | bash
```

**From source (requires Rust):**

```sh
git clone https://github.com/voila-dev/sitegrabber.git
cd sitegrabber
cargo build --release
cp target/release/sitegrabber /usr/local/bin/
```

## Usage

```sh
# Download a site into a ./example.com/ directory
sitegrabber https://example.com

# Specify output directory
sitegrabber https://example.com -o my-mirror

# Limit crawl depth and concurrency
sitegrabber https://docs.example.com -d 5 -c 4

# Include subdomains
sitegrabber https://example.com --include-subdomains

# Only download assets referenced by the entry page
sitegrabber https://example.com --assets-only
```

Then open the directory in your browser:

```sh
open example.com/index.html
```

Or serve it locally:

```sh
npx serve example.com
```

## Options

```
sitegrabber <URL> [OPTIONS]

  -o, --output <DIR>          Output directory (default: domain name)
  -d, --depth <N>             Max crawl depth (default: 25)
  -c, --concurrency <N>       Parallel downloads (default: 8)
  -t, --timeout <SECS>        Request timeout in seconds (default: 30)
      --user-agent <STRING>   Custom user-agent (default: SiteGrabber/1.0)
      --include-subdomains    Also crawl subdomains
      --assets-only           Only download assets, don't crawl pages
  -h, --help                  Print help
```

## What it does

1. Parses the starting URL and creates an output directory
2. Crawls all internal links (same domain) using an async work queue
3. Downloads every page and asset it finds — HTML, CSS, JS, images, fonts, videos
4. Extracts URLs from HTML attributes (`href`, `src`, `srcset`, `poster`, etc.), CSS `url()` and `@import`
5. Rewrites all internal URLs to relative paths so the site works from the filesystem
6. Prints a summary with page count, asset count, errors, and total size

## How paths work

URLs are mapped to filesystem paths that preserve the original structure:

| URL | File |
|---|---|
| `/` | `index.html` |
| `/about` | `about/index.html` |
| `/about/team` | `about/team/index.html` |
| `/css/style.css` | `css/style.css` |
| `/img/logo.png` | `img/logo.png` |

All links in HTML and CSS are rewritten to relative paths (e.g. `../../css/style.css`) so the site renders correctly when opened with `file://` or any static file server.

## Deploy to static hosting

The output directory is a complete static site. Upload it anywhere:

```sh
# Cloudflare Pages
npx wrangler pages deploy ./example.com

# Netlify
netlify deploy --dir=./example.com --prod

# GitHub Pages — just push the directory

# Any static server
rsync -av ./example.com/ user@server:/var/www/mirror/
```

## License

MIT
