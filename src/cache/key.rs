//! Stable cache key construction.

/// Current cache key schema version.
pub const CACHE_KEY_VERSION: &str = "toven-cache-v2";

/// Hex-encoded BLAKE3 cache key.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct CacheKey(String);

impl CacheKey {
    /// Create a cache key from a precomputed hex value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Return the hex-encoded key.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CacheKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Builder that length-prefixes fields before hashing to avoid ambiguity.
#[derive(Default)]
pub struct CacheKeyBuilder {
    fields: Vec<Vec<u8>>,
}

impl CacheKeyBuilder {
    /// Create an empty cache key builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a string field.
    #[must_use]
    pub fn field(mut self, value: impl AsRef<str>) -> Self {
        self.fields.push(value.as_ref().as_bytes().to_vec());
        self
    }

    /// Append all fields in sorted order.
    #[must_use]
    pub fn sorted_fields<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut values = values
            .into_iter()
            .map(|value| value.as_ref().as_bytes().to_vec())
            .collect::<Vec<_>>();
        values.sort();
        self.fields.extend(values);
        self
    }

    /// Build the final BLAKE3 key.
    #[must_use]
    pub fn build(self) -> CacheKey {
        let mut hasher = blake3::Hasher::new();
        append_field(&mut hasher, CACHE_KEY_VERSION.as_bytes());
        for field in self.fields {
            append_field(&mut hasher, &field);
        }
        CacheKey(hasher.finalize().to_hex().to_string())
    }
}

fn append_field(hasher: &mut blake3::Hasher, value: &[u8]) {
    hasher.update(&(value.len() as u64).to_be_bytes());
    hasher.update(value);
}

#[cfg(test)]
mod tests {
    use super::CacheKeyBuilder;

    #[test]
    fn length_prefixing_avoids_field_ambiguity() {
        let first = CacheKeyBuilder::new().field("a").field("bc").build();
        let second = CacheKeyBuilder::new().field("ab").field("c").build();

        assert_ne!(first, second);
    }

    #[test]
    fn sorted_fields_are_order_independent() {
        let first = CacheKeyBuilder::new().sorted_fields(["b", "a"]).build();
        let second = CacheKeyBuilder::new().sorted_fields(["a", "b"]).build();

        assert_eq!(first, second);
    }
}
