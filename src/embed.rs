//! Deterministic, keyless feature-hashed text embeddings.

/// Dimensionality of every embedding vector produced by [`embed`].
pub const EMBED_DIM: usize = 256;

/// Computes a deterministic, keyless 256-dimensional feature-hashed
/// embedding for `text`.
///
/// `text` is lowercased and split into maximal runs of alphanumeric
/// characters (tokens); tokens with fewer than 2 characters are dropped.
/// The feature set is every remaining token plus every adjacent token
/// bigram (`"{a} {b}"`). Each feature is hashed with FNV-1a; the hash picks
/// a bucket in `0..EMBED_DIM` and a `+1`/`-1` sign from its top bit, and the
/// sign is added into that bucket. The resulting vector is L2-normalized.
/// If `text` produced no features, the fallback unit vector with a `1.0` in
/// position 0 and `0.0` elsewhere is returned.
pub fn embed(text: &str) -> Vec<f32> {
    let lowercase = text.to_lowercase();
    let tokens: Vec<String> = tokenize(&lowercase)
        .into_iter()
        .filter(|token| token.chars().count() >= 2)
        .collect();

    let mut features: Vec<String> = tokens.clone();
    for pair in tokens.windows(2) {
        features.push(format!("{} {}", pair[0], pair[1]));
    }

    let mut vector = vec![0.0f32; EMBED_DIM];
    for feature in &features {
        let hash = fnv1a64(feature);
        let index = (hash % EMBED_DIM as u64) as usize;
        let sign = if (hash >> 63) == 0 { 1.0 } else { -1.0 };
        vector[index] += sign;
    }

    let norm = vector
        .iter()
        .map(|component| component * component)
        .sum::<f32>()
        .sqrt();
    if norm == 0.0 {
        vector[0] = 1.0;
        return vector;
    }
    for component in &mut vector {
        *component /= norm;
    }
    vector
}

/// Splits `text` into maximal runs of `char::is_alphanumeric` characters.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            current.push(ch);
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Computes the 64-bit FNV-1a hash of `s` over its UTF-8 bytes.
fn fnv1a64(s: &str) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    for byte in s.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// Computes the cosine distance `1 - dot(a, b)` between `a` and `b`.
///
/// # Preconditions
/// Both `a` and `b` must be L2-normalized (unit) vectors, such as those
/// produced by [`embed`]; the result is meaningful only under that
/// assumption. Vectors of unequal length are compared over their shorter
/// length rather than producing an error.
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    1.0 - dot
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_input_is_deterministic() {
        assert_eq!(
            embed("export controls on lithography tools"),
            embed("export controls on lithography tools")
        );
    }

    #[test]
    fn output_has_expected_dimension() {
        assert_eq!(
            embed("export controls on lithography tools").len(),
            EMBED_DIM
        );
    }

    #[test]
    fn output_is_unit_normalized() {
        let vector = embed("export controls on lithography tools");
        let norm = vector
            .iter()
            .map(|component| component * component)
            .sum::<f32>()
            .sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "norm was {norm}");
    }

    #[test]
    fn similar_text_is_closer_than_unrelated_text() {
        let export_controls = embed("export controls on lithography tools");
        let lithography_restrictions = embed("lithography tool export restrictions");
        let rice_harvest = embed("rice harvest in the Mekong delta");

        let similar_distance = cosine_distance(&export_controls, &lithography_restrictions);
        let unrelated_distance = cosine_distance(&export_controls, &rice_harvest);

        assert!(
            similar_distance < unrelated_distance,
            "similar={similar_distance} unrelated={unrelated_distance}"
        );
    }

    #[test]
    fn empty_string_yields_fallback_unit_vector() {
        let vector = embed("");
        let mut expected = vec![0.0f32; EMBED_DIM];
        expected[0] = 1.0;
        assert_eq!(vector, expected);
    }
}
