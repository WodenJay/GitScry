use crate::{DIMENSIONS, Error, Vector};

pub(crate) fn mean(values: &[f32], width: usize, lengths: &[usize]) -> Result<Vec<Vector>, Error> {
    if values.len() != lengths.len() * width * DIMENSIONS || values.iter().any(|v| !v.is_finite()) {
        return Err(Error::InvalidOutput("invalid token tensor".into()));
    }
    let mut vectors = Vec::with_capacity(lengths.len());
    for (row, &length) in lengths.iter().enumerate() {
        if length == 0 || length > width {
            return Err(Error::InvalidOutput("invalid attention mask".into()));
        }
        let mut sum = [0f64; DIMENSIONS];
        for token in 0..length {
            let start = (row * width + token) * DIMENSIONS;
            for (sum, value) in sum.iter_mut().zip(&values[start..start + DIMENSIONS]) {
                *sum += f64::from(*value);
            }
        }
        let mut vector = sum.map(|sum| (sum / length as f64) as f32);
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if !norm.is_finite() || norm <= 0.0 {
            return Err(Error::InvalidOutput("invalid pooled norm".into()));
        }
        for value in &mut vector {
            *value /= norm;
        }
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if vector.iter().any(|v| !v.is_finite()) || (norm - 1.0).abs() > 1e-5 {
            return Err(Error::InvalidOutput("invalid normalized vector".into()));
        }
        vectors.push(vector);
    }
    Ok(vectors)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_is_not_pooled_and_special_tokens_are() {
        let mut tokens = vec![0.0; 3 * DIMENSIONS];
        tokens[0] = 3.0;
        tokens[DIMENSIONS + 1] = 4.0;
        tokens[2 * DIMENSIONS] = 1e8;
        let vectors = mean(&tokens, 3, &[2]).unwrap();
        assert_eq!(vectors[0][0], 0.6);
        assert_eq!(vectors[0][1], 0.8);
    }

    #[test]
    fn malformed_and_nonfinite_outputs_are_rejected() {
        assert!(mean(&[0.0], 1, &[1]).is_err());
        assert!(mean(&[0.0; DIMENSIONS], 1, &[1]).is_err());
        assert!(mean(&[f32::NAN; DIMENSIONS], 1, &[1]).is_err());
        assert!(mean(&[f32::INFINITY; DIMENSIONS], 1, &[1]).is_err());
        assert!(mean(&[1.0; DIMENSIONS], 1, &[0]).is_err());
    }
}
