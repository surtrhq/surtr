use serde::{Deserialize, Serialize};

/// Where a value came from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Source {
    /// Provider that produced the value, for example `"linux"` or `"mock"`.
    pub provider: String,
    /// Mechanism inside the provider, for example `"cpuid"`, `"hwmon"` or `"nvml"`.
    pub method: String,
}

impl Source {
    /// Creates a source tag.
    pub fn new(provider: impl Into<String>, method: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            method: method.into(),
        }
    }

    /// Tags `value` with this source, ready to store in a [`Field`].
    pub fn tag<T>(&self, value: T) -> Field<T> {
        Some(Sourced {
            value,
            source: self.clone(),
        })
    }
}

/// A value together with the [`Source`] it came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sourced<T> {
    /// The value itself.
    pub value: T,
    /// Where the value came from.
    pub source: Source,
}

/// A hardware value that may be absent. `None` means "not known", never a
/// default or zero.
pub type Field<T> = Option<Sourced<T>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_keeps_value_and_source() {
        let src = Source::new("mock", "fixture");
        let field = src.tag(8u32).unwrap();
        assert_eq!(field.value, 8);
        assert_eq!(field.source, src);
    }

    #[test]
    fn absent_field_serializes_as_null() {
        let field: Field<u32> = None;
        assert_eq!(serde_json::to_string(&field).unwrap(), "null");
    }
}
