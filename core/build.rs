//! (nextvi C build disabled — its process-global state made repeated `vi`
//! invocations crash in our long-lived process. `vi` uses the stable Rust
//! modal editor instead. Kept for a future thread-localized integration.)
fn main() {}
