//! Local-first Cloudflare Durable Object spike (APS 13).
#[cfg(target_arch = "wasm32")]
mod durable;
#[cfg(target_arch = "wasm32")]
mod heap;
#[cfg(target_arch = "wasm32")]
mod storage;
