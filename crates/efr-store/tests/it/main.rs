//! The store's search index on a database file of the seeded corpus: the disk budget,
//! the search reads, and the benches of the writer time of the index and of the reads
//! (`bench_search_*`, ignored; `just bench-search` runs them in release, one at a
//! time). The `bench_perf_*` benches of the user messages and of one append on the
//! perf corpus (ignored; `just bench-perf` runs them).

#![cfg(test)]

mod perf;
mod search_bench;
mod search_query;
mod support;
