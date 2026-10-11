//! Errors for the content-addressed store.

use crate::Address;

/// What can go wrong.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CasError {
    /// Filesystem failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The address is not stored.
    #[error("missing blob {address}")]
    Missing {
        /// The absent address.
        address: Address,
    },

    /// A hex string was not a valid address.
    #[error("bad address {text:?}")]
    BadAddress {
        /// The offending text.
        text: String,
    },
}
