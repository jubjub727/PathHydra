use std::{error::Error, fmt};

use pathhydra_core::{EdgeId, NodeId};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SubgraphError {
    EdgeIdentityConflict {
        edge: EdgeId,
        existing_source: NodeId,
        existing_destination: NodeId,
        proposed_source: NodeId,
        proposed_destination: NodeId,
    },
    InvalidPath {
        reason: &'static str,
    },
}

impl fmt::Display for SubgraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EdgeIdentityConflict { edge, .. } => {
                write!(
                    formatter,
                    "edge {edge} was reused with different endpoint evidence"
                )
            }
            Self::InvalidPath { reason } => write!(formatter, "invalid route path: {reason}"),
        }
    }
}

impl Error for SubgraphError {}
