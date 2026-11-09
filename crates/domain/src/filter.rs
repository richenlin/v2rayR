//! Node list filtering.
//!
//! The search box in the nodes page is a tiny query language rather than a
//! substring match, because power users expect to narrow by protocol or port
//! without leaving the keyboard:
//!
//! ```text
//! tokio -expired          # free text, second term excludes
//! proto:vless port:443    # field filters
//! tag:stream              # user tag
//! ```
//!
//! All include terms must match (AND); any exclude term disqualifies. Matching
//! is case-insensitive and never allocates per node in the hot path beyond
//! lowercasing the query once.

use crate::model::Profile;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Term {
    Text(String),
    Protocol(String),
    Port(u16),
    Tag(String),
    Address(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeFilter {
    include: Vec<Term>,
    exclude: Vec<Term>,
}

impl NodeFilter {
    pub fn parse(query: &str) -> Self {
        let mut filter = Self::default();
        for raw in query.split_whitespace() {
            let (excluded, token) = match raw.strip_prefix('-') {
                Some(rest) => (true, rest),
                None => (false, raw),
            };
            if token.is_empty() {
                continue;
            }
            let term = parse_term(token);
            if excluded {
                filter.exclude.push(term);
            } else {
                filter.include.push(term);
            }
        }
        filter
    }

    pub fn is_empty(&self) -> bool {
        self.include.is_empty() && self.exclude.is_empty()
    }

    /// True when every include term matches and no exclude term does.
    pub fn matches(&self, profile: &Profile) -> bool {
        if self.is_empty() {
            return true;
        }
        let haystack = Haystack::new(profile);
        self.include.iter().all(|term| haystack.matches(term))
            && !self.exclude.iter().any(|term| haystack.matches(term))
    }
}

fn parse_term(token: &str) -> Term {
    if let Some((field, value)) = token.split_once(':') {
        match field.to_ascii_lowercase().as_str() {
            "proto" | "protocol" => return Term::Protocol(value.to_ascii_lowercase()),
            "port" => {
                if let Ok(port) = value.parse::<u16>() {
                    return Term::Port(port);
                }
            }
            "tag" => return Term::Tag(value.to_ascii_lowercase()),
            "ip" | "host" | "address" => return Term::Address(value.to_ascii_lowercase()),
            _ => {}
        }
    }
    Term::Text(token.to_ascii_lowercase())
}

struct Haystack {
    remark: String,
    address: String,
    protocol: String,
    tags: Vec<String>,
    port: u16,
    subscription: String,
}

impl Haystack {
    fn new(profile: &Profile) -> Self {
        Self {
            remark: profile.remark.to_ascii_lowercase(),
            address: profile.address.to_ascii_lowercase(),
            protocol: profile.protocol.kind().to_ascii_lowercase(),
            tags: profile
                .tags
                .iter()
                .map(|tag| tag.to_ascii_lowercase())
                .collect(),
            port: profile.port,
            subscription: profile.sub_id.map(|id| id.to_string()).unwrap_or_default(),
        }
    }

    fn matches(&self, term: &Term) -> bool {
        match term {
            Term::Text(needle) => {
                self.remark.contains(needle)
                    || self.address.contains(needle)
                    || self.protocol.contains(needle)
                    || self.tags.iter().any(|tag| tag.contains(needle))
                    || self.subscription.contains(needle)
            }
            Term::Protocol(value) => &self.protocol == value,
            Term::Port(port) => self.port == *port,
            Term::Tag(value) => self.tags.iter().any(|tag| tag == value),
            Term::Address(value) => self.address.contains(value),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Protocol, Secret};

    fn node(remark: &str, protocol: Protocol, port: u16) -> Profile {
        Profile::new(remark, "example.com", port, protocol)
    }

    fn trojan(remark: &str) -> Profile {
        node(
            remark,
            Protocol::Trojan {
                password: Secret::new("pw"),
            },
            443,
        )
    }

    #[test]
    fn empty_query_matches_everything() {
        assert!(NodeFilter::parse("   ").matches(&trojan("tokyo")));
    }

    #[test]
    fn text_is_case_insensitive() {
        assert!(NodeFilter::parse("TOK").matches(&trojan("Tokyo 01")));
        assert!(!NodeFilter::parse("osaka").matches(&trojan("Tokyo 01")));
    }

    #[test]
    fn exclusion_removes_matches() {
        let filter = NodeFilter::parse("tokyo -expired");
        assert!(filter.matches(&trojan("Tokyo 01")));
        assert!(!filter.matches(&trojan("Tokyo 01 expired")));
    }

    #[test]
    fn protocol_and_port_filters_compose() {
        let filter = NodeFilter::parse("proto:trojan port:443");
        assert!(filter.matches(&trojan("a")));
        let mut vless = trojan("a");
        vless.protocol = Protocol::Vless {
            id: uuid::Uuid::nil(),
            flow: None,
            encryption: None,
        };
        assert!(!filter.matches(&vless));
    }

    #[test]
    fn unknown_field_falls_back_to_text() {
        let filter = NodeFilter::parse("bare:metal");
        assert!(filter.matches(&trojan("bare:metal node")));
    }

    #[test]
    fn tag_filter_is_exact() {
        let mut profile = trojan("a");
        profile.tags = vec!["stream".to_owned(), "netflix".to_owned()];
        assert!(NodeFilter::parse("tag:stream").matches(&profile));
        assert!(!NodeFilter::parse("tag:streaming").matches(&profile));
    }
}
