use std::cell::Cell;
use std::io;
use std::path::Path;

use pretty_assertions::assert_eq;

use crate::surface::{ConfigLister, ConfigListings, MAX_CONFIG_LISTINGS_BYTES};

/// Lists every content as `k\n<content>`, and counts its runs.
#[derive(Default)]
struct Echo {
    runs: Cell<usize>,
}

impl ConfigLister for Echo {
    fn list(&self, _config: &Path) -> io::Result<String> {
        Err(io::Error::other("the listings list by content"))
    }

    fn list_content(&self, content: &[u8]) -> io::Result<String> {
        self.runs.set(self.runs.get() + 1);
        if content == b"bad" {
            return Err(io::Error::other("bad config"));
        }
        Ok(format!("k\n{}\0", String::from_utf8_lossy(content)))
    }
}

#[test]
fn the_file_keeps_the_used_listings_of_its_git_only() {
    let echo = Echo::default();
    let mut listings = ConfigListings::new("git a");
    assert_eq!(listings.listing(b"one", &echo), Some("k\none\0"));
    assert_eq!(listings.listing(b"two", &echo), Some("k\ntwo\0"));
    assert_eq!(listings.listing(b"one", &echo), Some("k\none\0"));
    assert_eq!(echo.runs.get(), 2);
    assert_eq!(listings.listing(b"bad", &echo), None);
    assert_eq!(listings.listing(b"bad", &echo), None);
    assert_eq!(echo.runs.get(), 4, "a failed listing is never kept");
    let json = listings.to_json();
    // The next call uses only `two`: the file it leaves has `two` alone.
    let mut next = ConfigListings::from_json(&json, "git a");
    assert_eq!(next.listing(b"two", &echo), Some("k\ntwo\0"));
    assert_eq!(echo.runs.get(), 4);
    let kept = String::from_utf8(next.to_json()).unwrap();
    assert!(kept.contains("two") && !kept.contains("one"), "{kept}");
    // Another git, a file that does not parse or one that is too large: no listings.
    for (bytes, git) in [
        (json.clone(), "git b"),
        (b"{not json".to_vec(), "git a"),
        (vec![b' '; MAX_CONFIG_LISTINGS_BYTES + 1], "git a"),
    ] {
        let mut found = ConfigListings::from_json(&bytes, git);
        let runs = echo.runs.get();
        found.listing(b"one", &echo);
        assert_eq!(echo.runs.get(), runs + 1);
    }
}

#[test]
fn the_file_leaves_out_what_is_not_utf8_or_does_not_fit() {
    let echo = Echo::default();
    let mut listings = ConfigListings::new("git");
    listings.listing(&[0xff, 0xfe], &echo);
    let big = "x".repeat(MAX_CONFIG_LISTINGS_BYTES / 4);
    listings.listing(big.as_bytes(), &echo);
    listings.listing(b"small", &echo);
    let json = listings.to_json();
    assert!(json.len() <= MAX_CONFIG_LISTINGS_BYTES);
    let mut next = ConfigListings::from_json(&json, "git");
    let runs = echo.runs.get();
    next.listing(b"small", &echo);
    assert_eq!(echo.runs.get(), runs, "the small content is kept");
    next.listing(big.as_bytes(), &echo);
    next.listing(&[0xff, 0xfe], &echo);
    assert_eq!(echo.runs.get(), runs + 2, "the others are listed again");
}
