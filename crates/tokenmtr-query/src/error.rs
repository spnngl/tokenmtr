//! Errors of the query layer.

use tokenmtr_core::MoneyError;
use tokenmtr_store::StoreError;

/// Why a query failed.
#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    /// The store refused a read.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// An SQL statement failed.
    #[error("sql: {0}")]
    Sql(#[from] rusqlite::Error),
    /// A stored value does not parse into its domain type.
    #[error("stored {what} {value:?} is invalid")]
    Decode {
        /// The column or concept.
        what: &'static str,
        /// The offending value.
        value: String,
    },
    /// Money arithmetic failed.
    #[error(transparent)]
    Money(#[from] MoneyError),
    /// A EUR fee has to become USD but no FX rate is known.
    #[error("no FX rate to convert {date}")]
    NoFxRate {
        /// The day that needed a rate.
        date: jiff::civil::Date,
    },
    /// The embedded plan catalog is broken.
    #[error(transparent)]
    Plans(#[from] tokenmtr_core::billing::PlanCatalogError),
    /// Calendar or time-zone arithmetic left the supported range.
    #[error("time: {0}")]
    Time(#[from] jiff::Error),
}

impl QueryError {
    #[expect(
        clippy::needless_pass_by_value,
        reason = "call sites hand over temporaries and owned strings"
    )]
    pub(crate) fn decode(what: &'static str, value: impl ToString) -> Self {
        Self::Decode {
            what,
            value: value.to_string(),
        }
    }
}

/// Why a [`crate::Filters`] or [`crate::DateRange`] could not be built from untrusted input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FilterError {
    /// The range is empty or inverted.
    #[error("range must end after it starts")]
    EmptyRange,
    /// A range of zero days was requested.
    #[error("a range needs at least one day")]
    ZeroDays,
    /// A calendar computation overflowed.
    #[error("date out of range")]
    OutOfRange,
    /// A filter value is not a valid identifier of its kind.
    #[error("invalid {kind} {value:?}")]
    BadValue {
        /// What the value should have been.
        kind: &'static str,
        /// The offending text.
        value: String,
    },
}
