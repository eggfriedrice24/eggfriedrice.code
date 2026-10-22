//! The store's search index on a database file of the seeded corpus: the disk budget,
//! and the benches of the writer time of the index (`bench_search_*`, ignored; `just
//! bench-search` runs them in release, one at a time).

#![cfg(test)]

mod search_bench;
mod support;
