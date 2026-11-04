//! Vanilla `Transformation.EXTENDED_CODEC`: a display's transformation in its decomposed form
//! or as a matrix, which `MatrixUtil.svdDecompose` splits up the way the vanilla server does.

use std::f32::consts::{SQRT_2, TAU};

use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};

/// Columns first, so `m[1][0]` is vanilla's `m10`.
type Matrix3 = [[f32; 3]; 3];
type Quaternion = [f32; 4];

const IDENTITY_ROTATION: Quaternion = [0.0, 0.0, 0.0, 1.0];
/// Vanilla `MatrixUtil.G`.
const G: f32 = 3.0 + 2.0 * SQRT_2;
const EPSILON: f32 = 1.0E-6;

/// The four parts vanilla keeps as a display's synced data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Transformation {
    pub translation: [f32; 3],
    pub left_rotation: Quaternion,
    pub scale: [f32; 3],
    pub right_rotation: Quaternion,
}

impl Transformation {
    /// Vanilla `Transformation.IDENTITY`.
    pub const IDENTITY: Self = Self {
        translation: [0.0, 0.0, 0.0],
        left_rotation: IDENTITY_ROTATION,
        scale: [1.0, 1.0, 1.0],
        right_rotation: IDENTITY_ROTATION,
    };

    /// Reads either form, `None` when the tag is neither (vanilla then falls back to the identity).
    pub fn from_nbt(tag: &NbtTag) -> Option<Self> {
        tag.extract_compound()
            .and_then(Self::from_compound)
            .or_else(|| float_list::<16>(tag).map(|matrix| Self::from_matrix(&matrix)))
    }

    /// Vanilla `Transformation.CODEC`: every field is required.
    fn from_compound(compound: &NbtCompound) -> Option<Self> {
        Some(Self {
            translation: float_list(compound.get("translation")?)?,
            left_rotation: quaternion_from_nbt(compound.get("left_rotation")?)?,
            scale: float_list(compound.get("scale")?)?,
            right_rotation: quaternion_from_nbt(compound.get("right_rotation")?)?,
        })
    }

    /// Vanilla `ExtraCodecs.MATRIX4F` lists rows first; `Transformation.ensureDecomposed` splits it.
    fn from_matrix(matrix: &[f32; 16]) -> Self {
        let entry = |row: usize, column: usize| matrix[row * 4 + column];
        let scale_factor = 1.0 / entry(3, 3);
        let upper: Matrix3 =
            std::array::from_fn(|c| std::array::from_fn(|r| entry(r, c) * scale_factor));
        let (left_rotation, scale, right_rotation) = svd_decompose(upper);
        Self {
            translation: std::array::from_fn(|row| entry(row, 3) * scale_factor),
            left_rotation,
            scale,
            right_rotation,
        }
    }

    /// Vanilla always saves the decomposed form.
    pub fn to_nbt(&self) -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.put_list("translation", float_tags(&self.translation));
        compound.put_list("left_rotation", float_tags(&self.left_rotation));
        compound.put_list("scale", float_tags(&self.scale));
        compound.put_list("right_rotation", float_tags(&self.right_rotation));
        compound
    }
}

fn float_tags(values: &[f32]) -> Vec<NbtTag> {
    values.iter().map(|&value| NbtTag::Float(value)).collect()
}

/// Vanilla `Util.fixedSize` over a `Codec.FLOAT` list, which takes any number.
fn float_list<const N: usize>(tag: &NbtTag) -> Option<[f32; N]> {
    let list = tag.extract_list()?;
    if list.len() != N {
        return None;
    }
    let mut values = [0.0f32; N];
    for (value, element) in values.iter_mut().zip(list) {
        *value = element.as_numeric_float()?;
    }
    Some(values)
}

/// Vanilla `ExtraCodecs.QUATERNIONF`: four normalized components, or an angle and an axis.
fn quaternion_from_nbt(tag: &NbtTag) -> Option<Quaternion> {
    if let Some(components) = float_list::<4>(tag) {
        return Some(normalize(components));
    }
    let compound = tag.extract_compound()?;
    let angle = compound.get_numeric_float("angle")?;
    let [x, y, z] = float_list::<3>(compound.get("axis")?)?;
    // JOML `AxisAngle4f` wraps the angle, `Quaternionf` leaves the axis as it is.
    let (sin, cos) = (angle.rem_euclid(TAU) * 0.5).sin_cos();
    Some([x * sin, y * sin, z * sin, cos])
}

fn normalize(q: Quaternion) -> Quaternion {
    let inverse_norm = 1.0 / (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    q.map(|component| component * inverse_norm)
}

/// JOML `Quaternionf.mul`.
const fn multiply_quaternions(a: Quaternion, b: Quaternion) -> Quaternion {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// JOML `Matrix3f.mul`: `a` times `b`.
fn multiply_matrices(a: &Matrix3, b: &Matrix3) -> Matrix3 {
    std::array::from_fn(|c| {
        std::array::from_fn(|r| a[0][r] * b[c][0] + a[1][r] * b[c][1] + a[2][r] * b[c][2])
    })
}

fn transpose(m: &Matrix3) -> Matrix3 {
    std::array::from_fn(|c| std::array::from_fn(|r| m[r][c]))
}

/// The rotation JOML `Matrix3f.rotate(Quaternionfc)` multiplies with.
const fn rotation_matrix(q: Quaternion) -> Matrix3 {
    let [x, y, z, w] = q;
    let (x2, y2, z2, w2) = (x * x, y * y, z * z, w * w);
    let (dxy, dxz, dyz) = (2.0 * x * y, 2.0 * x * z, 2.0 * y * z);
    let (dxw, dyw, dzw) = (2.0 * x * w, 2.0 * y * w, 2.0 * z * w);
    [
        [w2 + x2 - z2 - y2, dxy + dzw, dxz - dyw],
        [dxy - dzw, y2 - z2 + w2 - x2, dyz + dxw],
        [dyw + dxz, dyz - dxw, z2 - y2 - x2 + w2],
    ]
}

/// Vanilla `GivensParameters`: half a rotation around one axis.
#[derive(Clone, Copy)]
struct GivensParameters {
    sin_half: f32,
    cos_half: f32,
}

impl GivensParameters {
    fn from_unnormalized(sin_half: f32, cos_half: f32) -> Self {
        let inverse_norm = 1.0 / (sin_half * sin_half + cos_half * cos_half).sqrt();
        Self {
            sin_half: inverse_norm * sin_half,
            cos_half: inverse_norm * cos_half,
        }
    }

    /// Vanilla `MatrixUtil.PI_4`.
    fn quarter_turn() -> Self {
        let (sin_half, cos_half) = std::f32::consts::FRAC_PI_8.sin_cos();
        Self { sin_half, cos_half }
    }

    const fn inverse(self) -> Self {
        Self {
            sin_half: -self.sin_half,
            cos_half: self.cos_half,
        }
    }

    const fn cos(self) -> f32 {
        self.cos_half * self.cos_half - self.sin_half * self.sin_half
    }

    const fn sin(self) -> f32 {
        2.0 * self.sin_half * self.cos_half
    }

    const fn quaternion_x(self) -> Quaternion {
        [self.sin_half, 0.0, 0.0, self.cos_half]
    }

    const fn quaternion_y(self) -> Quaternion {
        [0.0, self.sin_half, 0.0, self.cos_half]
    }

    const fn quaternion_z(self) -> Quaternion {
        [0.0, 0.0, self.sin_half, self.cos_half]
    }

    const fn matrix_x(self) -> Matrix3 {
        let (c, s) = (self.cos(), self.sin());
        [[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]]
    }

    const fn matrix_y(self) -> Matrix3 {
        let (c, s) = (self.cos(), self.sin());
        [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]]
    }

    const fn matrix_z(self) -> Matrix3 {
        let (c, s) = (self.cos(), self.sin());
        [[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]]
    }
}

/// Vanilla `MatrixUtil.approxGivensQuat`.
fn approx_givens_quat(a11: f32, a12: f32, a22: f32) -> GivensParameters {
    let ch = 2.0 * (a11 - a22);
    if G * a12 * a12 < ch * ch {
        GivensParameters::from_unnormalized(a12, ch)
    } else {
        GivensParameters::quarter_turn()
    }
}

/// Vanilla `MatrixUtil.qrGivensQuat`.
fn qr_givens_quat(a1: f32, a2: f32) -> GivensParameters {
    let p = a1.hypot(a2);
    let sh = if p > EPSILON { a2 } else { 0.0 };
    let ch = a1.abs() + p.max(EPSILON);
    if a1 < 0.0 {
        GivensParameters::from_unnormalized(ch, sh)
    } else {
        GivensParameters::from_unnormalized(sh, ch)
    }
}

/// Vanilla `MatrixUtil.similarityTransform`.
fn similarity_transform(a: &Matrix3, q: &Matrix3) -> Matrix3 {
    multiply_matrices(&transpose(q), &multiply_matrices(a, q))
}

/// Vanilla `MatrixUtil.stepJacobi`.
fn step_jacobi(m: &mut Matrix3, output: &mut Quaternion) {
    if m[0][1] * m[0][1] + m[1][0] * m[1][0] > EPSILON {
        let p = approx_givens_quat(m[0][0], 0.5 * (m[0][1] + m[1][0]), m[1][1]);
        *output = multiply_quaternions(*output, p.quaternion_z());
        *m = similarity_transform(m, &p.matrix_z());
    }
    if m[0][2] * m[0][2] + m[2][0] * m[2][0] > EPSILON {
        let p = approx_givens_quat(m[0][0], 0.5 * (m[0][2] + m[2][0]), m[2][2]).inverse();
        *output = multiply_quaternions(*output, p.quaternion_y());
        *m = similarity_transform(m, &p.matrix_y());
    }
    if m[1][2] * m[1][2] + m[2][1] * m[2][1] > EPSILON {
        let p = approx_givens_quat(m[1][1], 0.5 * (m[1][2] + m[2][1]), m[2][2]);
        *output = multiply_quaternions(*output, p.quaternion_x());
        *m = similarity_transform(m, &p.matrix_x());
    }
}

/// Vanilla `MatrixUtil.svdDecompose`: left rotation, scale and right rotation of `input`.
fn svd_decompose(input: Matrix3) -> (Quaternion, [f32; 3], Quaternion) {
    let mut scratch = multiply_matrices(&transpose(&input), &input);
    let mut v = IDENTITY_ROTATION;
    for _ in 0..5 {
        step_jacobi(&mut scratch, &mut v);
    }
    let v = normalize(v);
    let zero_column_0 = f64::from(scratch[0][0]) < 1.0E-6;
    let zero_column_1 = f64::from(scratch[1][1]) < 1.0E-6;

    let u012s = multiply_matrices(&input, &rotation_matrix(v));
    let p = if zero_column_0 {
        qr_givens_quat(u012s[1][1], -u012s[1][0])
    } else {
        qr_givens_quat(u012s[0][0], u012s[0][1])
    };
    let mut u = multiply_quaternions(IDENTITY_ROTATION, p.quaternion_z());
    let u12s = multiply_matrices(&transpose(&p.matrix_z()), &u012s);

    let p = if zero_column_0 {
        qr_givens_quat(u12s[2][2], -u12s[2][0])
    } else {
        qr_givens_quat(u12s[0][0], u12s[0][2])
    }
    .inverse();
    u = multiply_quaternions(u, p.quaternion_y());
    let u2s = multiply_matrices(&transpose(&p.matrix_y()), &u12s);

    let p = if zero_column_1 {
        qr_givens_quat(u2s[2][2], -u2s[2][1])
    } else {
        qr_givens_quat(u2s[1][1], u2s[1][2])
    };
    u = multiply_quaternions(u, p.quaternion_x());
    let sm = multiply_matrices(&transpose(&p.matrix_x()), &u2s);

    let conjugate_v = [-v[0], -v[1], -v[2], v[3]];
    (u, [sm[0][0], sm[1][1], sm[2][2]], conjugate_v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matrix_tag(values: [f32; 16]) -> NbtTag {
        NbtTag::List(values.iter().map(|&v| NbtTag::Float(v)).collect())
    }

    fn assert_close(actual: &[f32], expected: &[f32]) {
        for (a, e) in actual.iter().zip(expected) {
            assert!((a - e).abs() < 1.0E-4, "{actual:?} is not {expected:?}");
        }
    }

    #[test]
    fn a_translated_scale_matrix_splits_into_its_parts() {
        let transformation = Transformation::from_nbt(&matrix_tag([
            2.0, 0.0, 0.0, 5.0, //
            0.0, 3.0, 0.0, 6.0, //
            0.0, 0.0, 4.0, 7.0, //
            0.0, 0.0, 0.0, 1.0,
        ]))
        .unwrap_or(Transformation::IDENTITY);
        assert_close(&transformation.translation, &[5.0, 6.0, 7.0]);
        assert_close(&transformation.scale, &[2.0, 3.0, 4.0]);
        assert_close(&transformation.left_rotation, &IDENTITY_ROTATION);
        assert_close(&transformation.right_rotation, &IDENTITY_ROTATION);
    }

    /// The bottom right entry divides the whole matrix, as in `MatrixUtil.svdDecompose`.
    #[test]
    fn the_last_matrix_entry_scales_everything() {
        let transformation = Transformation::from_nbt(&matrix_tag([
            2.0, 0.0, 0.0, 4.0, //
            0.0, 2.0, 0.0, 0.0, //
            0.0, 0.0, 2.0, 0.0, //
            0.0, 0.0, 0.0, 2.0,
        ]))
        .unwrap_or(Transformation::IDENTITY);
        assert_close(&transformation.translation, &[2.0, 0.0, 0.0]);
        assert_close(&transformation.scale, &[1.0, 1.0, 1.0]);
    }

    /// Left rotation, scale and right rotation multiply back to the matrix they came from.
    #[test]
    fn a_decomposed_matrix_composes_back() {
        // A quarter turn around Z, then a shear-free stretch along X.
        let columns: Matrix3 = [[0.0, 2.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
        let (u, s, v) = svd_decompose(columns);
        let scale = [[s[0], 0.0, 0.0], [0.0, s[1], 0.0], [0.0, 0.0, s[2]]];
        let composed = multiply_matrices(
            &multiply_matrices(&rotation_matrix(u), &scale),
            &rotation_matrix(v),
        );
        for (column, expected) in composed.iter().zip(&columns) {
            assert_close(column, expected);
        }
    }

    #[test]
    fn rotations_are_normalized_components_or_an_angle_and_axis() {
        let components = NbtTag::List(float_tags(&[0.0, 0.0, 0.0, 2.0]));
        assert_eq!(quaternion_from_nbt(&components), Some(IDENTITY_ROTATION));

        let mut axis_angle = NbtCompound::new();
        axis_angle.put_float("angle", std::f32::consts::PI);
        axis_angle.put_list("axis", float_tags(&[0.0, 1.0, 0.0]));
        let rotation = quaternion_from_nbt(&NbtTag::Compound(axis_angle));
        assert_close(&rotation.unwrap_or_default(), &[0.0, 1.0, 0.0, 0.0]);
    }

    /// `Transformation.CODEC` wants all four fields; anything less falls back to the identity.
    #[test]
    fn a_partial_transformation_is_rejected() {
        let mut compound = NbtCompound::new();
        compound.put_list("scale", float_tags(&[2.0, 2.0, 2.0]));
        assert_eq!(Transformation::from_nbt(&NbtTag::Compound(compound)), None);
    }
}
