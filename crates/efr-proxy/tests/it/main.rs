#![cfg(test)]
//! The proxy over real Unix sockets and loopback servers, with the test seams of
//! `kit`: no real host, no real resolver.

mod budget;
mod connect;
mod kit;
mod limits;
mod plain;
