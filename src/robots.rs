use reqwest::Client;
use url::Url;

pub struct RobotsRules {
    disallow: Vec<String>,
    allow: Vec<String>,
}

impl RobotsRules {
    pub fn allow_all() -> Self {
        RobotsRules {
            disallow: Vec::new(),
            allow: Vec::new(),
        }
    }

    pub fn is_allowed(&self, path: &str) -> bool {
        // Check allow rules first (more specific wins, allow takes precedence on tie)
        let mut dominated_by_allow = false;
        let mut best_allow_len = 0;
        for rule in &self.allow {
            if path_matches(path, rule) && rule.len() >= best_allow_len {
                dominated_by_allow = true;
                best_allow_len = rule.len();
            }
        }

        let mut dominated_by_disallow = false;
        let mut best_disallow_len = 0;
        for rule in &self.disallow {
            if path_matches(path, rule) && rule.len() >= best_disallow_len {
                dominated_by_disallow = true;
                best_disallow_len = rule.len();
            }
        }

        if !dominated_by_disallow {
            return true;
        }
        if dominated_by_allow && best_allow_len >= best_disallow_len {
            return true;
        }
        false
    }
}

fn path_matches(path: &str, rule: &str) -> bool {
    if rule.ends_with('*') {
        path.starts_with(&rule[..rule.len() - 1])
    } else if rule.ends_with('$') {
        path == &rule[..rule.len() - 1]
    } else {
        path.starts_with(rule)
    }
}

pub async fn fetch_robots(client: &Client, base_url: &Url, user_agent: &str) -> RobotsRules {
    let robots_url = format!("{}://{}/robots.txt", base_url.scheme(), base_url.host_str().unwrap_or(""));

    let body = match client.get(&robots_url).send().await {
        Ok(resp) if resp.status().is_success() => {
            resp.text().await.unwrap_or_default()
        }
        _ => return RobotsRules::allow_all(),
    };

    parse_robots(&body, user_agent)
}

fn parse_robots(body: &str, user_agent: &str) -> RobotsRules {
    let ua_lower = user_agent.split('/').next().unwrap_or(user_agent).to_lowercase();
    let mut disallow = Vec::new();
    let mut allow = Vec::new();
    let mut in_matching_group = false;
    let mut found_specific = false;

    // First pass: look for our specific user-agent
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }

        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().to_lowercase();
            let value = value.trim();

            match key.as_str() {
                "user-agent" => {
                    let agent = value.to_lowercase();
                    if agent == ua_lower || agent == "*" {
                        in_matching_group = true;
                        if agent == ua_lower {
                            found_specific = true;
                        }
                    } else {
                        in_matching_group = false;
                    }
                }
                "disallow" if in_matching_group && !value.is_empty() => {
                    disallow.push(value.to_string());
                }
                "allow" if in_matching_group && !value.is_empty() => {
                    allow.push(value.to_string());
                }
                _ => {}
            }
        }
    }

    // If we found specific rules, use them; otherwise the wildcard rules are already collected
    if !found_specific {
        // Re-parse for wildcard only
        disallow.clear();
        allow.clear();
        in_matching_group = false;

        for line in body.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_lowercase();
                let value = value.trim();
                match key.as_str() {
                    "user-agent" => {
                        in_matching_group = value.trim() == "*";
                    }
                    "disallow" if in_matching_group && !value.is_empty() => {
                        disallow.push(value.to_string());
                    }
                    "allow" if in_matching_group && !value.is_empty() => {
                        allow.push(value.to_string());
                    }
                    _ => {}
                }
            }
        }
    }

    RobotsRules { disallow, allow }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- path_matches ---

    #[test]
    fn path_matches_prefix() {
        assert!(path_matches("/admin/settings", "/admin/"));
        assert!(path_matches("/admin", "/admin"));
        assert!(!path_matches("/administrator", "/admin/"));
    }

    #[test]
    fn path_matches_wildcard_suffix() {
        assert!(path_matches("/images/photo.jpg", "/images/*"));
        assert!(path_matches("/images/", "/images/*"));
        assert!(!path_matches("/img/photo.jpg", "/images/*"));
    }

    #[test]
    fn path_matches_exact_with_dollar() {
        assert!(path_matches("/secret", "/secret$"));
        assert!(!path_matches("/secret/page", "/secret$"));
    }

    #[test]
    fn path_matches_root() {
        assert!(path_matches("/", "/"));
        assert!(path_matches("/anything", "/"));
    }

    // --- RobotsRules::is_allowed ---

    #[test]
    fn allow_all_allows_everything() {
        let rules = RobotsRules::allow_all();
        assert!(rules.is_allowed("/"));
        assert!(rules.is_allowed("/admin"));
        assert!(rules.is_allowed("/secret/page"));
    }

    #[test]
    fn disallow_single_path() {
        let rules = RobotsRules {
            disallow: vec!["/admin/".to_string()],
            allow: vec![],
        };
        assert!(!rules.is_allowed("/admin/settings"));
        assert!(!rules.is_allowed("/admin/"));
        assert!(rules.is_allowed("/about"));
        assert!(rules.is_allowed("/"));
    }

    #[test]
    fn disallow_all() {
        let rules = RobotsRules {
            disallow: vec!["/".to_string()],
            allow: vec![],
        };
        assert!(!rules.is_allowed("/"));
        assert!(!rules.is_allowed("/anything"));
    }

    #[test]
    fn allow_overrides_disallow_when_more_specific() {
        let rules = RobotsRules {
            disallow: vec!["/admin/".to_string()],
            allow: vec!["/admin/public/".to_string()],
        };
        assert!(!rules.is_allowed("/admin/settings"));
        assert!(rules.is_allowed("/admin/public/page"));
    }

    #[test]
    fn allow_overrides_disallow_on_equal_length() {
        let rules = RobotsRules {
            disallow: vec!["/page".to_string()],
            allow: vec!["/page".to_string()],
        };
        assert!(rules.is_allowed("/page"));
    }

    #[test]
    fn multiple_disallow_rules() {
        let rules = RobotsRules {
            disallow: vec!["/admin/".to_string(), "/private/".to_string()],
            allow: vec![],
        };
        assert!(!rules.is_allowed("/admin/x"));
        assert!(!rules.is_allowed("/private/y"));
        assert!(rules.is_allowed("/public/z"));
    }

    // --- parse_robots ---

    #[test]
    fn parse_robots_wildcard_agent() {
        let body = "\
User-agent: *
Disallow: /admin/
Disallow: /private/
Allow: /admin/public/
";
        let rules = parse_robots(body, "SiteGrabber/1.0");
        assert!(!rules.is_allowed("/admin/settings"));
        assert!(rules.is_allowed("/admin/public/page"));
        assert!(!rules.is_allowed("/private/data"));
        assert!(rules.is_allowed("/"));
    }

    #[test]
    fn parse_robots_specific_agent() {
        let body = "\
User-agent: *
Disallow: /

User-agent: SiteGrabber
Disallow: /secret/
Allow: /
";
        let rules = parse_robots(body, "SiteGrabber/1.0");
        // Should use SiteGrabber-specific rules, not wildcard
        assert!(rules.is_allowed("/public"));
        assert!(!rules.is_allowed("/secret/stuff"));
    }

    #[test]
    fn parse_robots_no_matching_agent_uses_wildcard() {
        let body = "\
User-agent: Googlebot
Disallow: /google-only/

User-agent: *
Disallow: /private/
";
        let rules = parse_robots(body, "SiteGrabber/1.0");
        assert!(rules.is_allowed("/google-only/"));
        assert!(!rules.is_allowed("/private/data"));
    }

    #[test]
    fn parse_robots_empty_body() {
        let rules = parse_robots("", "SiteGrabber/1.0");
        assert!(rules.is_allowed("/anything"));
    }

    #[test]
    fn parse_robots_comments_and_blanks() {
        let body = "\
# This is a comment
User-agent: *

# Another comment
Disallow: /secret/
";
        let rules = parse_robots(body, "SiteGrabber/1.0");
        assert!(!rules.is_allowed("/secret/page"));
        assert!(rules.is_allowed("/public"));
    }

    #[test]
    fn parse_robots_empty_disallow_means_allow_all() {
        let body = "\
User-agent: *
Disallow:
";
        let rules = parse_robots(body, "SiteGrabber/1.0");
        assert!(rules.is_allowed("/anything"));
    }
}
