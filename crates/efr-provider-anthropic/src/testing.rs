//! Fakes shared by the unit tests of this crate: a clock whose sleeps end at once, a
//! token source of fixed keys, the fixture loader of `fixtures/`, and fake
//! `POST /messages` and `GET /models` answers on `wiremock`. No test calls the real API.
