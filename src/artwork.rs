//! Album artwork lookup.
//!
//! Chromium-based players (YouTube Music in a browser) do not report the
//! album name through SMTC, so the cover art is looked up via the free
//! Apple iTunes Search API (no key, https URLs that Discord accepts
//! directly as external Rich Presence assets).

use std::collections::HashMap;

const SEARCH_URL: &str = "https://itunes.apple.com/search";

/// Pure lookup result cache. Key: `artist|album|title`.
#[derive(Default)]
pub struct ArtworkCache {
    results: HashMap<String, Option<String>>,
}

impl ArtworkCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the cached URL for this track, or `None` on a miss.
    pub fn get(&self, artist: &str, album: &str, title: &str) -> Option<&String> {
        self.results.get(&cache_key(artist, album, title))?.as_ref()
    }

    /// Remembers the lookup outcome (a miss is also cached).
    pub fn put(&mut self, artist: &str, album: &str, title: &str, url: Option<String>) {
        self.results.insert(cache_key(artist, album, title), url);
    }
}

fn cache_key(artist: &str, album: &str, title: &str) -> String {
    format!("{artist}|{album}|{title}")
}

/// iTunes serves `/100x100bb.jpg`; Discord is happy with bigger art,
/// so swap to 600x600.
pub fn upscale_artwork_url(url: &str) -> String {
    if let Some(rest) = url.strip_suffix("/100x100bb.jpg") {
        return format!("{rest}/600x600bb.jpg");
    }
    if let Some(rest) = url.strip_suffix("/100x100bb.png") {
        return format!("{rest}/600x600bb.png");
    }
    url.to_string()
}

/// Candidate search terms, best first: album search, then song search.
pub fn search_terms(artist: &str, album: &str, title: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if !artist.is_empty() && !album.is_empty() {
        out.push((format!("{artist} {album}"), "album".to_string()));
    }
    if !artist.is_empty() && !title.is_empty() {
        out.push((format!("{artist} {title}"), "song".to_string()));
    }
    if artist.is_empty() {
        if !album.is_empty() {
            out.push((album.to_string(), "album".to_string()));
        }
        if !title.is_empty() {
            out.push((title.to_string(), "song".to_string()));
        }
    }
    out
}

/// Picks the best result from an iTunes JSON response body.
///
/// Prefers a result whose artist/album/collection loosely matches the
/// query, falling back to the first entry. Pure and unit-testable.
pub fn pick_best_result(body: &str, artist: &str, album: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let results = value.get("results")?.as_array()?;
    let score = |r: &serde_json::Value| -> usize {
        let r_artist = r.get("artistName").and_then(|v| v.as_str()).unwrap_or("");
        let r_collection = r
            .get("collectionName")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let r_track = r.get("trackName").and_then(|v| v.as_str()).unwrap_or("");
        let mut score = 0;
        if contains_ignore_case(r_artist, artist) {
            score += 2;
        }
        if contains_ignore_case(r_collection, album) {
            score += 2;
        }
        if contains_ignore_case(r_track, album) {
            score += 1;
        }
        score
    };
    results
        .iter()
        .max_by_key(|r| score(r))
        .or_else(|| results.first())
        .and_then(|r| {
            r.get("artworkUrl100")
                .and_then(|v| v.as_str())
                .map(ToString::to_string)
        })
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    !needle.is_empty() && haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Runs the actual lookup. Blocking HTTP; called from the poller thread.
pub fn find_artwork_url(
    agent: &ureq::Agent,
    artist: &str,
    album: &str,
    title: &str,
) -> Option<String> {
    for (term, entity) in search_terms(artist, album, title) {
        let response = agent
            .get(SEARCH_URL)
            .query("term", &term)
            .query("media", "music")
            .query("entity", &entity)
            .query("limit", "5")
            .call();
        let Ok(mut response) = response else {
            continue;
        };
        let body = response.body_mut().read_to_string().ok()?;
        if let Some(found) = pick_best_result(&body, artist, album) {
            return Some(upscale_artwork_url(&found));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upscale_swaps_size_suffix() {
        assert_eq!(
            upscale_artwork_url("https://is1-ssl.mzstatic.com/x/100x100bb.jpg"),
            "https://is1-ssl.mzstatic.com/x/600x600bb.jpg"
        );
    }

    #[test]
    fn upscale_leaves_unknown_urls_alone() {
        assert_eq!(
            upscale_artwork_url("https://example.com/art.png"),
            "https://example.com/art.png"
        );
    }

    #[test]
    fn search_terms_album_first_then_song() {
        let terms = search_terms("Daft Punk", "Discovery", "Aerodynamic");
        assert_eq!(terms.len(), 2);
        assert_eq!(terms[0], ("Daft Punk Discovery".into(), "album".into()));
        assert_eq!(terms[1], ("Daft Punk Aerodynamic".into(), "song".into()));
    }

    #[test]
    fn search_terms_falls_back_without_artist() {
        let terms = search_terms("", "Discovery", "Aerodynamic");
        assert_eq!(terms.len(), 2);
        assert_eq!(terms[0].1, "album");
        assert_eq!(terms[1].1, "song");
    }

    #[test]
    fn pick_best_result_prefers_matching_artist() {
        let body = r#"{"results":[
            {"artistName":"Someone Else","collectionName":"Discovery","artworkUrl100":"https://a/100x100bb.jpg"},
            {"artistName":"Daft Punk","collectionName":"Discovery","artworkUrl100":"https://b/100x100bb.jpg"}
        ]}"#;
        let picked = pick_best_result(body, "Daft Punk", "Discovery");
        assert_eq!(picked.as_deref(), Some("https://b/100x100bb.jpg"));
    }

    #[test]
    fn pick_best_result_falls_back_to_first() {
        let body =
            r#"{"results":[{"artistName":"X","artworkUrl100":"https://first/100x100bb.jpg"}]}"#;
        let picked = pick_best_result(body, "Daft Punk", "Discovery");
        assert_eq!(picked.as_deref(), Some("https://first/100x100bb.jpg"));
    }

    #[test]
    fn pick_best_result_empty_response() {
        assert_eq!(pick_best_result("{}", "a", "b"), None);
        assert_eq!(pick_best_result("not json", "a", "b"), None);
    }

    #[test]
    fn cache_stores_misses_too() {
        let mut cache = ArtworkCache::new();
        cache.put("a", "b", "c", None);
        assert!(cache.get("a", "b", "c").is_none());
        cache.put("a", "b", "c", Some("u".into()));
        assert_eq!(cache.get("a", "b", "c"), Some(&"u".to_string()));
    }
}

/// Live check of the real iTunes API, run manually: `cargo test -- --ignored live`.
#[cfg(test)]
mod live_tests {
    use super::*;

    #[test]
    #[ignore = "network test, run manually"]
    fn live_itunes_lookup_returns_artwork() {
        let agent = ureq::Agent::new_with_config(ureq::config::Config::default());
        let url = find_artwork_url(&agent, "Daft Punk", "Discovery", "Aerodynamic");
        let url = url.expect("expected a result for Daft Punk - Discovery");
        assert!(url.starts_with("https://"));
        assert!(url.contains("600x600bb"));
    }
}
