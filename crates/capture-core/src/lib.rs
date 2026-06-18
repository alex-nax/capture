//! capture-core — the v3 contract layer.
//!
//! Defines the serde types for the `/v1` HTTP API (requests + responses) and the on-disk
//! session formats. These replace the v2 pydantic `daemon/models.py` + the `v1_schema` golden
//! as the SOURCE OF TRUTH for the contract firewall: the GUI (and the future Rust daemon / MCP)
//! depend on these, so the wire + on-disk shapes stay byte-identical across the incremental port.
//!
//! See `docs/specs/v3-architecture.md`. The contract type modules (`v1` requests/responses,
//! `ondisk` session formats) land in #61's type-port phase.

/// The `/v1` API version this contract describes (matches `HealthResponse.api_version`).
pub const API_VERSION: u32 = 1;
