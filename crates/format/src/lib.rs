//! Versioned, deterministic, bounded document packages (decisions 05, 21, 34).
//! See `SPEC.md` for the wire format and compatibility rules.

mod assets;
mod container;
mod image_assets;
mod json;
mod migration;
mod package;
#[cfg(test)]
mod tests;

pub use assets::{
    Asset, AssetAvailability, AssetKind, AssetNeed, AssetSource, ExternalLocation, FontPin,
    content_hash,
};
pub use container::{
    CURRENT_VERSION, Container, DocumentId, FeatureFlags, FormatError, Header, Limits, Section, ids,
};
pub use migration::{Migration, MigrationRegistry, MigrationStep};
pub use package::{CacheContext, CacheTags, DerivedCache, OpenedFile, Package, SnapshotReference};

/// Diagnostic codes are public and stable. Fatal failures use `FormatError`.
pub mod codes {
    use reprise_diag::Code;
    pub const INVALID: Code = Code::new("format.invalid");
    pub const CACHE_DROPPED: Code = Code::new("format.cache-dropped");
    pub const CACHE_IGNORED: Code = Code::new("format.cache-ignored");
    pub const ASSET_HASH: Code = Code::new("format.asset-hash");
    pub const FONT_HASH: Code = Code::new("format.font-hash");
    pub const FONT_MISSING: Code = Code::new("format.font-missing");
    pub const FONT_UNREADABLE: Code = Code::new("format.font-unreadable");
    pub const ASSET_MISSING: Code = Code::new("format.asset-missing");
    pub const MIGRATED: Code = Code::new("format.migrated");
    pub const READ_ONLY: Code = Code::new("format.read-only");
    pub const LIMIT: Code = Code::new("format.limit");
}
