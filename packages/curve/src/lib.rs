#![no_std]

use core::cmp::Ordering;

use soroban_sdk::{vec, Env, Vec};

/// Handle Contract Errors
#[derive(Debug, Eq, PartialEq)]
pub enum CurveError {
    /// A monotonic function is a function between ordered sets that preserves
    /// or reverses the given order, but never both.
    // "Curve isn't monotonic"
    NotMonotonic,

    /// A curve that always grows or stay constant
    // "Curve is monotonic increasing"
    MonotonicIncreasing,

    /// A curve that always decrease or stay constant
    // "Curve is monotonic decreasing"
    MonotonicDecreasing,

    /// Fail on Monotonic increasing or decreasing
    // "Later point must have higher X than previous point"
    PointsOutOfOrder,

    /// No curve points defined
    // "No steps defined"
    MissingSteps,

    /// The resulting curve would become too complex.
    /// Prevents vesting curves from becoming too complex, rendering the account useless.
    // "Curve is too complex"
    TooComplex,
}

/// Curve types
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Curve {
    /// Constan curve, it will always have the same value
    Constant {
        /// Contanst value y
        y: u128,
    },
    /// Linear curve that grow linearly but later
    /// tends to a constant saturated value.
    SaturatingLinear(SaturatingLinear),

    /// Curve with different slopes
    PiecewiseLinear(PiecewiseLinear),
}

impl Curve {
    /// Ctor for Saturated curve
    pub fn saturating_linear((min_x, min_y): (u64, u128), (max_x, max_y): (u64, u128)) -> Self {
        Curve::SaturatingLinear(SaturatingLinear {
            min_x,
            min_y: min_y.into(),
            max_x,
            max_y: max_y.into(),
        })
    }

    /// Ctor for constant curve
    pub fn constant(y: u128) -> Self {
        Curve::Constant { y }
    }
}

impl Curve {
    /// provides y = f(x) evaluation
    pub fn value(&self, x: u64) -> u128 {
        match self {
            Curve::Constant { y } => *y,
            Curve::SaturatingLinear(s) => s.value(x),
            Curve::PiecewiseLinear(p) => p.value(x),
        }
    }

    /// returns the number of steps in the curve
    pub fn size(&self) -> u32 {
        match self {
            Curve::Constant { .. } => 1,
            Curve::SaturatingLinear(_) => 2,
            Curve::PiecewiseLinear(pl) => pl.steps.len(),
        }
    }

    /// general sanity checks on input values to ensure this is valid.
    /// these checks should be included by the validate_monotonic_* functions
    pub fn validate(&self) -> Result<(), CurveError> {
        match self {
            Curve::Constant { .. } => Ok(()),
            Curve::SaturatingLinear(s) => s.validate(),
            Curve::PiecewiseLinear(p) => p.validate(),
        }
    }

    /// returns an error if there is ever x2 > x1 such that value(x2) < value(x1)
    pub fn validate_monotonic_increasing(&self) -> Result<(), CurveError> {
        match self {
            Curve::Constant { .. } => Ok(()),
            Curve::SaturatingLinear(s) => s.validate_monotonic_increasing(),
            Curve::PiecewiseLinear(p) => p.validate_monotonic_increasing(),
        }
    }

    /// returns an error if there is ever x2 > x1 such that value(x1) < value(x2)
    pub fn validate_monotonic_decreasing(&self) -> Result<(), CurveError> {
        match self {
            Curve::Constant { .. } => Ok(()),
            Curve::SaturatingLinear(s) => s.validate_monotonic_decreasing(),
            Curve::PiecewiseLinear(p) => p.validate_monotonic_decreasing(),
        }
    }

    /// returns an error if the size of the curve is more than the given max.
    pub fn validate_complexity(&self, max: u32) -> Result<(), CurveError> {
        if self.size() <= max {
            Ok(())
        } else {
            Err(CurveError::TooComplex)
        }
    }

    /// return (min, max) that can ever be returned from value. These could potentially be u128::MIN and u128::MAX
    pub fn range(&self) -> (u128, u128) {
        match self {
            Curve::Constant { y } => (*y, *y),
            Curve::SaturatingLinear(sat) => sat.range(),
            Curve::PiecewiseLinear(p) => p.range(),
        }
    }

    /// combines a constant with a curve (shifting the curve up)
    fn combine_const(&self, env: &Env, const_y: u128) -> Curve {
        match self {
            Curve::Constant { y } => Curve::Constant { y: const_y + y },
            Curve::SaturatingLinear(sl) => Curve::SaturatingLinear(SaturatingLinear {
                min_x: sl.min_x,
                min_y: sl.min_y + const_y,
                max_x: sl.max_x,
                max_y: sl.max_y + const_y,
            }),
            Curve::PiecewiseLinear(pl) => {
                let mut new_steps = soroban_sdk::Vec::new(&env);

                for i in 0..pl.steps.len() {
                    let (x, y) = pl.steps.get(i).unwrap().unwrap();
                    new_steps.push_back((x, const_y + y));
                }
                Curve::PiecewiseLinear(PiecewiseLinear { steps: new_steps })
            }
        }
    }

    /// returns a new curve that is the result of adding the given curve to this one
    pub fn combine(&self, env: &Env, other: &Curve) -> Curve {
        match (self, other) {
            // special handling for constant cases:
            (Curve::Constant { y }, curve) | (curve, Curve::Constant { y }) => {
                curve.combine_const(&env, *y)
            }
            // cases that can be converted to piecewise linear:
            (Curve::SaturatingLinear(sl1), Curve::SaturatingLinear(sl2)) => {
                // convert to piecewise linear, then combine those
                Curve::PiecewiseLinear(
                    PiecewiseLinear::from(sl1).combine(&PiecewiseLinear::from(sl2)),
                )
            }
            (Curve::SaturatingLinear(sl), Curve::PiecewiseLinear(pl))
            | (Curve::PiecewiseLinear(pl), Curve::SaturatingLinear(sl)) => {
                // convert sl to piecewise linear, then combine
                Curve::PiecewiseLinear(PiecewiseLinear::from(sl).combine(pl))
            }
            (Curve::PiecewiseLinear(pl1), Curve::PiecewiseLinear(pl2)) => {
                Curve::PiecewiseLinear(pl1.combine(pl2))
            }
        }
    }
}

/// Saturating Linear
/// $$f(x)=\begin{cases}
/// [min(y) * amount],  & \text{if x <= $x_1$ } \\\\
/// [y * amount],  & \text{if $x_1$ >= x <= $x_2$ } \\\\
/// [max(y) * amount],  & \text{if x >= $x_2$ }
/// \end{cases}$$
///
/// min_y for all x <= min_x, max_y for all x >= max_x, linear in between
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SaturatingLinear {
    /// time when curve start
    pub min_x: u64,
    // I would use u128, but those cause parse error, which was fixed in https://github.com/CosmWasm/serde-json-wasm/pull/37
    // but not yet released in serde-wasm-json v0.4.0
    /// min value at start time
    pub min_y: u128,
    /// time when curve has fully saturated
    pub max_x: u64,
    /// max value at saturated time
    pub max_y: u128,
}

impl SaturatingLinear {
    /// provides y = f(x) evaluation
    pub fn value(&self, x: u64) -> u128 {
        match (x < self.min_x, x > self.max_x) {
            (true, _) => self.min_y,
            (_, true) => self.max_y,
            _ => interpolate((self.min_x, self.min_y), (self.max_x, self.max_y), x),
        }
    }

    /// general sanity checks on input values to ensure this is valid.
    /// these checks should be included by the other validate_* functions
    pub fn validate(&self) -> Result<(), CurveError> {
        if self.max_x <= self.min_x {
            return Err(CurveError::PointsOutOfOrder);
        }
        Ok(())
    }

    /// returns an error if there is ever x2 > x1 such that value(x2) < value(x1)
    pub fn validate_monotonic_increasing(&self) -> Result<(), CurveError> {
        self.validate()?;
        if self.max_y < self.min_y {
            return Err(CurveError::MonotonicDecreasing);
        }
        Ok(())
    }

    /// returns an error if there is ever x2 > x1 such that value(x1) < value(x2)
    pub fn validate_monotonic_decreasing(&self) -> Result<(), CurveError> {
        self.validate()?;
        if self.max_y > self.min_y {
            return Err(CurveError::MonotonicIncreasing);
        }
        Ok(())
    }

    /// return (min, max) that can ever be returned from value. These could potentially be 0 and u64::MAX
    pub fn range(&self) -> (u128, u128) {
        if self.max_y > self.min_y {
            (self.min_y, self.max_y)
        } else {
            (self.max_y, self.min_y)
        }
    }
}

// this requires min_x < x < max_x to have been previously validated
fn interpolate((min_x, min_y): (u64, u128), (max_x, max_y): (u64, u128), x: u64) -> u128 {
    if max_y > min_y {
        min_y + (max_y - min_y) * u128::from(x - min_x) / u128::from(max_x - min_x)
    } else {
        min_y - (min_y - max_y) * u128::from(x - min_x) / u128::from(max_x - min_x)
    }
}

/// This is a generalization of SaturatingLinear, steps must be arranged with increasing time [`u64`].
/// Any point before first step gets the first value, after last step the last value.
/// Otherwise, it is a linear interpolation between the two closest points.
/// Vec of length 1 -> [`Constant`](Curve::Constant) .
/// Vec of length 2 -> [`SaturatingLinear`] .
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct PiecewiseLinear {
    /// steps
    pub steps: Vec<(u64, u128)>,
}

impl PiecewiseLinear {
    /// provides y = f(x) evaluation
    pub fn value(&self, x: u64) -> u128 {
        let mut iter = self.steps.iter();

        let mut next = match iter.next() {
            Some(Ok(val)) => val,
            _ => panic!("Steps are empty or error in reading steps"), // or handle this situation differently
        };

        let mut prev: Option<&(u64, u128)> = None;

        for step_result in iter {
            // only break if x is not above prev
            if x >= next.0 {
                prev = Some(&next);
                next = step_result.unwrap();
            } else {
                break;
            }
        }

        // at this time:
        // prev may be None (this was lower than first point)
        // x may equal prev.0 (use this value)
        // x may be greater than next (if higher than last item)
        // OR x may be between prev and next (interpolate)
        if let Some(last) = prev {
            if x == last.0 {
                last.1
            } else if x >= next.0 {
                next.1
            } else {
                interpolate(*last, next, x)
            }
        } else {
            next.1
        }
    }

    // pub fn value(&self, x: u64) -> u128 {
    //     // figure out the pair of points it lies between
    //     let (mut prev, mut next): (Option<&(u64, u128)>, _) = (None, &self.steps[0]);
    //     for step in &self.steps[1..] {
    //         // only break if x is not above prev
    //         if x >= next.0 {
    //             prev = Some(next);
    //             next = step;
    //         } else {
    //             break;
    //         }
    //     }
    //     // at this time:
    //     // prev may be None (this was lower than first point)
    //     // x may equal prev.0 (use this value)
    //     // x may be greater than next (if higher than last item)
    //     // OR x may be between prev and next (interpolate)
    //     if let Some(last) = prev {
    //         if x == last.0 {
    //             // this handles exact match with low end
    //             last.1
    //         } else if x >= next.0 {
    //             // this handles both higher than all and exact match
    //             next.1
    //         } else {
    //             // here we do linear interpolation
    //             interpolate(*last, *next, x)
    //         }
    //     } else {
    //         // lower than all, use first
    //         next.1
    //     }
    // }

    /// general sanity checks on input values to ensure this is valid.
    /// these checks should be included by the other validate_* functions
    pub fn validate(&self) -> Result<(), CurveError> {
        if self.steps.is_empty() {
            return Err(CurveError::MissingSteps);
        }
        self.steps.iter().fold(Ok(0u64), |acc, step_result| {
            acc.and_then(|last| {
                let (x, _) = step_result.unwrap();
                if x > last {
                    Ok(x)
                } else {
                    Err(CurveError::PointsOutOfOrder)
                }
            })
        })?;
        Ok(())
    }

    /// returns an error if there is ever x2 > x1 such that value(x2) < value(x1)
    pub fn validate_monotonic_increasing(&self) -> Result<(), CurveError> {
        self.validate()?;
        match self.classify_curve() {
            Shape::NotMonotonic => Err(CurveError::NotMonotonic),
            Shape::MonotonicDecreasing => Err(CurveError::MonotonicDecreasing),
            _ => Ok(()),
        }
    }

    /// returns an error if there is ever x2 > x1 such that value(x1) < value(x2)
    pub fn validate_monotonic_decreasing(&self) -> Result<(), CurveError> {
        self.validate()?;
        match self.classify_curve() {
            Shape::NotMonotonic => Err(CurveError::NotMonotonic),
            Shape::MonotonicIncreasing => Err(CurveError::MonotonicIncreasing),
            _ => Ok(()),
        }
    }

    // Gives monotonic info. Requires there be at least one item in steps
    fn classify_curve(&self) -> Shape {
        let mut iter = self.steps.iter();
        let (_, first) = iter.next().unwrap().unwrap();
        let (_, shape) = iter.fold((first, Shape::Constant), |(last, shape), step_result| {
            let (_, y) = step_result.unwrap();
            let shape = match (shape, y.cmp(&last)) {
                (Shape::NotMonotonic, _) => Shape::NotMonotonic,
                (Shape::MonotonicDecreasing, Ordering::Greater) => Shape::NotMonotonic,
                (Shape::MonotonicDecreasing, _) => Shape::MonotonicDecreasing,
                (Shape::MonotonicIncreasing, Ordering::Less) => Shape::NotMonotonic,
                (Shape::MonotonicIncreasing, _) => Shape::MonotonicIncreasing,
                (Shape::Constant, Ordering::Greater) => Shape::MonotonicIncreasing,
                (Shape::Constant, Ordering::Less) => Shape::MonotonicDecreasing,
                (Shape::Constant, Ordering::Equal) => Shape::Constant,
            };
            (y, shape)
        });
        shape
    }

    /// return (min, max) that can ever be returned from value. These could potentially be 0 and u64::MAX
    pub fn range(&self) -> (u128, u128) {
        let low = self
            .steps
            .iter()
            .map(|step_result| {
                let (_, y) = step_result.unwrap();
                y
            })
            .min()
            .unwrap();
        let high = self
            .steps
            .iter()
            .map(|step_result| {
                let (_, y) = step_result.unwrap();
                y
            })
            .max()
            .unwrap();
        (low, high)
    }

    pub fn combine(&self, other: &PiecewiseLinear) -> PiecewiseLinear {
        // collect x-coordinates for combined curve
        let mut x: soroban_sdk::Vec<_> = self
            .steps
            .iter()
            .chain(other.steps.iter())
            .map(|step_result| {
                let (x, _) = step_result.unwrap();
                x
            })
            .collect::<soroban_sdk::Vec<_>>();

        // sort and deduplicate the vector
        // Using bubble sort as an example, but you should use a more efficient sorting algorithm
        let len = x.len();
        for i in 0..len {
            for j in 0..len - i - 1 {
                if x[j] > x[j + 1] {
                    let tmp = x[j];
                    x[j] = x[j + 1];
                    x[j + 1] = tmp;
                }
            }
        }

        // deduplication
        let mut i = 0;
        while i < x.len() - 1 {
            if x[i] == x[i + 1] {
                x.remove(i);
            } else {
                i += 1;
            }
        }

        // map to full coordinates
        PiecewiseLinear {
            steps: x
                .into_iter()
                .map(|x| (x, self.value(x) + other.value(x)))
                .collect::<soroban_sdk::Vec<_>>(),
        }
    }


    /// adds two piecewise linear curves and returns the result
    // pub fn combine(&self, other: &PiecewiseLinear) -> PiecewiseLinear {
    //     // collect x-coordinates for combined curve
    //     let mut x: Vec<_> = self
    //         .steps
    //         .iter()
    //         .chain(other.steps.iter())
    //         .map(|step_result| {
    //             let (x, _) = step_result.unwrap();
    //             x
    //         })
    //         .collect();
    //     x.sort_unstable();
    //     x.dedup();

    //     // map to full coordinates
    //     PiecewiseLinear {
    //         steps: x
    //             .into_iter()
    //             .map(|x| (x, self.value(x) + other.value(x)))
    //             .collect(),
    //     }
    // }
}

impl From<&SaturatingLinear> for PiecewiseLinear {
    fn from(sl: &SaturatingLinear) -> Self {
        PiecewiseLinear {
            steps: vec![(sl.min_x, sl.min_y), (sl.max_x, sl.max_y)],
        }
    }
}

enum Shape {
    // If there is only one point, or all have same value
    Constant,
    MonotonicIncreasing,
    MonotonicDecreasing,
    NotMonotonic,
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(524u128; "init constant y curve, should always return y")]
    fn test_constant(y: u128) {
        let curve = Curve::constant(y);

        // always valid
        curve.validate().unwrap();
        curve.validate_monotonic_increasing().unwrap();
        curve.validate_monotonic_decreasing().unwrap();

        // always returns same value
        assert_eq!(curve.value(1), y);
        assert_eq!(curve.value(1000000), y);

        // range is constant
        assert_eq!(curve.range(), (y, y));
    }

    #[test_case((100u64,0u128),(200u64,50u128); "test increasing linear, should monotonically increase linearly")]
    fn test_increasing_linear(low: (u64, u128), high: (u64, u128)) {
        let curve = Curve::saturating_linear(low, high);

        // validly increasing
        curve.validate().unwrap();
        curve.validate_monotonic_increasing().unwrap();
        // but not decreasing
        let err = curve.validate_monotonic_decreasing().unwrap_err();
        assert_eq!(err, CurveError::MonotonicIncreasing);

        // check extremes
        assert_eq!(curve.value(1), low.1);
        assert_eq!(curve.value(1000000), high.1);
        // check linear portion
        assert_eq!(curve.value(150), 25);
        // and rounding
        assert_eq!(curve.value(103), 1);

        // range is min to max
        assert_eq!(curve.range(), (low.1, high.1));
    }

    //TODO: This case and the previous can be done in one
    #[test_case((1700u64,500u128),(2000u64,200u128); "test decreasing linear, should monotonically decrease linearly")]
    fn test_decreasing_linear(low: (u64, u128), high: (u64, u128)) {
        let curve = Curve::saturating_linear(low, high);

        // validly decreasing
        curve.validate().unwrap();
        curve.validate_monotonic_decreasing().unwrap();
        // but not increasing
        let err = curve.validate_monotonic_increasing().unwrap_err();
        assert_eq!(err, CurveError::MonotonicDecreasing);

        // check extremes
        assert_eq!(curve.value(low.0 - 5), low.1);
        assert_eq!(curve.value(high.0 + 5), high.1);
        // check linear portion
        assert_eq!(curve.value(1800), 400);
        assert_eq!(curve.value(1997), 203);

        // range is min to max
        assert_eq!(curve.range(), (high.1, low.1));
    }

    //TODO: We should capture panic on test_case
    #[test_case((15000u64,100u128),(12000u64,200u128); "test invalid linear, should panic")]
    fn test_invalid_linear(low: (u64, u128), high: (u64, u128)) {
        let curve = Curve::saturating_linear(low, high);

        // always invalid
        let err = curve.validate().unwrap_err();
        assert_eq!(CurveError::PointsOutOfOrder, err);
        let err = curve.validate_monotonic_decreasing().unwrap_err();
        assert_eq!(CurveError::PointsOutOfOrder, err);
        let err = curve.validate_monotonic_increasing().unwrap_err();
        assert_eq!(CurveError::PointsOutOfOrder, err);
    }

    #[test_case(524u128; "test piecewise one step, should always return y")]
    fn test_piecewise_one_step(y: u128) {
        let curve = Curve::PiecewiseLinear(PiecewiseLinear {
            steps: vec![(12345, u128::new(y))],
        });

        // always valid
        curve.validate().unwrap();
        curve.validate_monotonic_increasing().unwrap();
        curve.validate_monotonic_decreasing().unwrap();

        // always returns same value
        assert_eq!(curve.value(1), y);
        assert_eq!(curve.value(1000000), y);

        // range is constant
        assert_eq!(curve.range(), (y, y));
    }

    #[test_case((100u64,u128::new(0)),(200u64,u128::new(50)); "test piecewise two point increasing, should not fail")]
    fn test_piecewise_two_point_increasing(low: (u64, u128), high: (u64, u128)) {
        let curve = Curve::PiecewiseLinear(PiecewiseLinear {
            steps: vec![low, high],
        });

        // validly increasing
        curve.validate().unwrap();
        curve.validate_monotonic_increasing().unwrap();
        // but not decreasing
        let err = curve.validate_monotonic_decreasing().unwrap_err();
        assert_eq!(err, CurveError::MonotonicIncreasing);

        // check extremes
        assert_eq!(curve.value(1), low.1);
        assert_eq!(curve.value(1000000), high.1);
        // check linear portion
        assert_eq!(curve.value(150), 25);
        // and rounding
        assert_eq!(curve.value(103), 1);
        // check both edges
        assert_eq!(curve.value(low.0), low.1);
        assert_eq!(curve.value(high.0), high.1);

        // range is min to max
        assert_eq!(curve.range(), (low.1, high.1));
    }

    #[test_case((1700u64,u128::new(500)),(2000u64,u128::new(200)); "test piecewise two point decreasing, should not fail")]
    fn test_piecewise_two_point_decreasing(low: (u64, u128), high: (u64, u128)) {
        let curve = Curve::PiecewiseLinear(PiecewiseLinear {
            steps: vec![low, high],
        });

        // validly decreasing
        curve.validate().unwrap();
        curve.validate_monotonic_decreasing().unwrap();
        // but not increasing
        let err = curve.validate_monotonic_increasing().unwrap_err();
        assert_eq!(err, CurveError::MonotonicDecreasing);

        // check extremes
        assert_eq!(curve.value(low.0 - 5), low.1);
        assert_eq!(curve.value(high.0 + 5), high.1);
        // check linear portion
        assert_eq!(curve.value(1800), 400);
        assert_eq!(curve.value(1997), 203);
        // check edge matches
        assert_eq!(curve.value(low.0), low.1);
        assert_eq!(curve.value(high.0), high.1);

        // range is min to max
        assert_eq!(curve.range(), (high.1, low.1));
    }

    #[test_case((15000u64,100u128),(12000u64,200u128); "test piecewise two point invalid, should fail")]
    fn test_piecewise_two_point_invalid(low: (u64, u128), high: (u64, u128)) {
        let curve = Curve::saturating_linear(low, high);

        // always invalid
        let err = curve.validate().unwrap_err();
        assert_eq!(CurveError::PointsOutOfOrder, err);
        let err = curve.validate_monotonic_decreasing().unwrap_err();
        assert_eq!(CurveError::PointsOutOfOrder, err);
        let err = curve.validate_monotonic_increasing().unwrap_err();
        assert_eq!(CurveError::PointsOutOfOrder, err);
    }

    #[test_case((100,u128::new(0)),(200,u128::new(100)),(300,u128::new(400)); "test piecewise two point invalid, should not fail")]
    fn test_piecewise_three_point_increasing(
        low: (u64, u128),
        mid: (u64, u128),
        high: (u64, u128),
    ) {
        let curve = Curve::PiecewiseLinear(PiecewiseLinear {
            steps: vec![low, mid, high],
        });

        // validly increasing
        curve.validate().unwrap();
        curve.validate_monotonic_increasing().unwrap();
        // but not decreasing
        let err = curve.validate_monotonic_decreasing().unwrap_err();
        assert_eq!(err, CurveError::MonotonicIncreasing);

        // check extremes
        assert_eq!(curve.value(1), low.1);
        assert_eq!(curve.value(1000000), high.1);

        // check first portion
        assert_eq!(curve.value(172), 72);
        // check second portion (100 + 3 * 40) = 220
        assert_eq!(curve.value(240), 220);

        // check all exact matches
        assert_eq!(curve.value(low.0), low.1);
        assert_eq!(curve.value(mid.0), mid.1);
        assert_eq!(curve.value(high.0), high.1);

        // range is min to max
        assert_eq!(curve.range(), (low.1, high.1));
    }

    #[test_case((100,u128::new(400)),(200,u128::new(100)),(300,u128::new(0)); "test piecewise three point decreasing, should not fail")]
    fn test_piecewise_three_point_decreasing(
        low: (u64, u128),
        mid: (u64, u128),
        high: (u64, u128),
    ) {
        let curve = Curve::PiecewiseLinear(PiecewiseLinear {
            steps: vec![low, mid, high],
        });

        // validly decreasing
        curve.validate().unwrap();
        curve.validate_monotonic_decreasing().unwrap();
        // but not increasing
        let err = curve.validate_monotonic_increasing().unwrap_err();
        assert_eq!(err, CurveError::MonotonicDecreasing);

        // check extremes
        assert_eq!(curve.value(1), low.1);
        assert_eq!(curve.value(1000000), high.1);

        // check first portion (400 - 72 * 3 = 184)
        assert_eq!(curve.value(172), 184);
        // check second portion (100 + 45) = 55
        assert_eq!(curve.value(245), 55);

        // check all exact matches
        assert_eq!(curve.value(low.0), low.1);
        assert_eq!(curve.value(mid.0), mid.1);
        assert_eq!(curve.value(high.0), high.1);

        // range is min to max
        assert_eq!(curve.range(), (high.1, low.1));
    }

    #[test_case((100,u128::new(400)),(200,u128::new(100)),(300,u128::new(300)); "test piecewise three point invalid not monotonic, should fail")]
    fn test_piecewise_three_point_invalid_not_monotonic(
        low: (u64, u128),
        mid: (u64, u128),
        high: (u64, u128),
    ) {
        let curve = Curve::PiecewiseLinear(PiecewiseLinear {
            steps: vec![low, mid, high],
        });

        // validly order
        curve.validate().unwrap();
        // not monotonic
        let err = curve.validate_monotonic_increasing().unwrap_err();
        assert_eq!(err, CurveError::NotMonotonic);
        // not increasing
        let err = curve.validate_monotonic_decreasing().unwrap_err();
        assert_eq!(err, CurveError::NotMonotonic);
    }

    // TODO: We can refactor this test based on the previous, changing the mid and high values on the previous one
    #[test_case((100,u128::new(400)),(200,u128::new(100)),(300,u128::new(300)); "test piecewise three point invalid out of order, should fail")]
    fn test_piecewise_three_point_invalid_out_of_order(
        low: (u64, u128),
        mid: (u64, u128),
        high: (u64, u128),
    ) {
        let curve = Curve::PiecewiseLinear(PiecewiseLinear {
            steps: vec![low, high, mid],
        });

        // validly order
        let err = curve.validate().unwrap_err();
        assert_eq!(err, CurveError::PointsOutOfOrder);
        // not monotonic
        let err = curve.validate_monotonic_increasing().unwrap_err();
        assert_eq!(err, CurveError::PointsOutOfOrder);
        // not increasing
        let err = curve.validate_monotonic_decreasing().unwrap_err();
        assert_eq!(err, CurveError::PointsOutOfOrder);
    }

    // TODO: multi-step bad

    #[test]
    fn test_saturating_to_piecewise() {
        let sl = SaturatingLinear {
            min_x: 15,
            min_y: u128::new(1),
            max_x: 60,
            max_y: u128::new(120),
        };
        let pw = PiecewiseLinear {
            steps: vec![(15, u128::new(1)), (60, u128::new(120))],
        };

        let converted = PiecewiseLinear::from(&sl);

        // should be the same
        assert_eq!(converted, pw);

        // check it still produces the same values
        for x in [0, 20, 60, 80] {
            assert_eq!(converted.value(x), sl.value(x));
        }
    }

    fn test_combine<const LEN: usize>(
        curve1: &Curve,
        curve2: &Curve,
        x_values: [u64; LEN],
        expected_size: usize,
    ) {
        let combined = curve1.combine(curve2);

        assert_eq!(
            combined,
            curve2.combine(curve1),
            "combine should be commutative"
        );

        // check some values
        for x in x_values {
            assert_eq!(combined.value(x), curve1.value(x) + curve2.value(x));
        }

        assert_eq!(combined.size(), expected_size);
    }

    #[test]
    fn test_combine_curves() {
        let c = Curve::Constant { y: u128::new(10) };
        let sl = Curve::SaturatingLinear(SaturatingLinear {
            min_x: 10,
            min_y: u128::new(10),
            max_x: 110,
            max_y: u128::new(210),
        });
        let pl = Curve::PiecewiseLinear(PiecewiseLinear {
            steps: vec![
                (10, u128::new(50)),
                (20, u128::new(70)),
                (30, u128::new(100)),
            ],
        });

        test_combine(&sl, &c, [0, 10, 20, 50, 100, 110, 120], 2);
        test_combine(&pl, &c, [0, 10, 15, 20, 25, 30, 35], 3);

        // deduplication for x-coordinate 10 expected, so only size 4
        test_combine(&pl, &sl, [0, 5, 10, 15, 20, 25, 30, 35, 60, 110], 4);

        // all points should be dedpulicated in these cases
        test_combine(&c, &c, [0, 5, 10, 15, 20], 1);
        test_combine(&pl, &pl, [0, 10, 15, 20, 25, 30, 35], 3);
        test_combine(&sl, &sl, [0, 10, 20, 50, 100, 110, 120], 2);
    }

    #[test]
    fn test_complexity_validation() {
        let curve = Curve::constant(6);
        assert_eq!(
            curve.validate_complexity(0).unwrap_err(),
            CurveError::TooComplex
        );
        curve.validate_complexity(1).unwrap();

        let curve = Curve::saturating_linear((0, 10), (1, 20));
        assert_eq!(
            curve.validate_complexity(1).unwrap_err(),
            CurveError::TooComplex
        );
        curve.validate_complexity(2).unwrap();

        let curve = Curve::PiecewiseLinear(PiecewiseLinear {
            steps: vec![(0, u128::new(0)), (10, u128::new(10)), (20, u128::new(20))],
        });

        assert_eq!(
            curve.validate_complexity(2).unwrap_err(),
            CurveError::TooComplex
        );
        curve.validate_complexity(3).unwrap();
        curve.validate_complexity(4).unwrap();
    }
}
