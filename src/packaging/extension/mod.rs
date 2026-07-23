//! Static discovery of Osiris interfaces installed in Python distributions.
//!
//! Discovery is data-only; dependency solving and installation remain uv's job.

include!("model.rs");
include!("discovery.rs");
include!("io.rs");
include!("validation.rs");
